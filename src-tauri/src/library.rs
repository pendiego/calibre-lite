//! calibre-compatible library: `metadata.db` (official schema) + `Author/Title (id)/` folders.
use crate::metadata::Candidate;
use rusqlite::{functions::FunctionFlags, params, Connection};
use serde::Serialize;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

type R<T> = Result<T, String>;
fn s<E: ToString>(e: E) -> String {
    e.to_string()
}

pub const ADD_EXTENSIONS: [&str; 15] = [
    "epub", "kepub", "pdf", "mobi", "azw", "azw3", "fb2", "cbz", "cbr", "djvu", "txt", "rtf",
    "docx", "html", "htm",
];
const UNKNOWN_AUTHOR: &str = "Desconhecido";
const PATH_LIMIT: usize = 40;
const UNDEFINED_DATE: &str = "0101-01-01 00:00:00+00:00";

// ---------- naming helpers ----------

fn sanitize_file_name(name: &str) -> String {
    let t: String = name
        .chars()
        .map(|c| {
            if "\\|?*<\":>+/".contains(c) || c.is_control() {
                '_'
            } else if c.is_whitespace() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let t = t.trim();
    let (stem, ext) = match t.rfind('.') {
        Some(i) => (&t[..i], &t[i..]),
        None => (t, ""),
    };
    let stem = if !stem.is_empty() && stem.chars().all(|c| c == '.') {
        "_".to_string()
    } else {
        stem.replace("..", "_")
    };
    let mut chars: Vec<char> = format!("{stem}{ext}").chars().collect();
    if let Some(l) = chars.last_mut() {
        if *l == '.' || *l == ' ' {
            *l = '_';
        }
    }
    if let Some(f) = chars.first_mut() {
        if *f == '.' {
            *f = '_';
        }
    }
    chars.into_iter().collect()
}

pub fn ascii_filename(name: &str) -> String {
    let t: String = deunicode::deunicode(name)
        .chars()
        .map(|c| if c == '?' || (c as u32) < 32 { '_' } else { c })
        .collect();
    sanitize_file_name(&t)
}

fn trunc(t: &str, n: usize) -> String {
    t.chars().take(n).collect()
}

fn is_reserved(a: &str) -> bool {
    let u = a.to_uppercase();
    matches!(u.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((u.starts_with("COM") || u.starts_with("LPT"))
            && u.len() == 4
            && u[3..].chars().all(|c| ('1'..='9').contains(&c)))
}

pub fn construct_path_name(id: i64, title: &str, author: &str) -> String {
    let suffix = format!(" ({id})");
    let l = PATH_LIMIT.saturating_sub(suffix.len() / 2 + 2);
    let author = trunc(&ascii_filename(author), l);
    let mut title = trunc(&ascii_filename(title.trim_start()), l)
        .trim_end()
        .to_string();
    if title.is_empty() {
        title = "Unknown".into();
    }
    let mut author = author.trim_end_matches([' ', '.']).to_string();
    if author.is_empty() {
        author = ascii_filename(UNKNOWN_AUTHOR);
    }
    if is_reserved(&author) {
        author.push('w');
    }
    format!("{author}/{title}{suffix}")
}

pub fn construct_file_name(_id: i64, title: &str, author: &str, extlen: usize) -> String {
    let extlen = extlen.max(14);
    let l = PATH_LIMIT.saturating_sub(extlen / 2 + 2);
    let author = trunc(&ascii_filename(author), l);
    let title = trunc(&ascii_filename(title.trim_start()), l).trim_end().to_string();
    let name = format!("{title} - {author}");
    let name = name.trim_end_matches('.');
    if name.is_empty() {
        ascii_filename(UNKNOWN_AUTHOR)
    } else {
        name.to_string()
    }
}

// ponytail: eng+por articles only; calibre picks per-language lists
pub fn title_sort(t: &str) -> String {
    let mut t = t.trim();
    for q in ['"', '\''] {
        if t.len() >= 2 && t.starts_with(q) && t.ends_with(q) {
            t = &t[1..t.len() - 1];
        }
    }
    if let Some((first, rest)) = t.split_once(char::is_whitespace) {
        let rest = rest.trim_start();
        const ARTICLES: [&str; 10] = [
            "a", "the", "an", "o", "os", "as", "um", "uns", "uma", "umas",
        ];
        if !rest.is_empty() && ARTICLES.contains(&first.to_lowercase().as_str()) {
            return format!("{rest}, {first}").trim().to_string();
        }
    }
    t.trim().to_string()
}

pub fn author_sort(name: &str) -> String {
    if name.contains(',') {
        return name.to_string();
    }
    let mut toks: Vec<&str> = name.split_whitespace().collect();
    if toks.len() < 2 {
        return name.to_string();
    }
    let mut suffix = None;
    let norm = |t: &str| t.to_lowercase().replace('.', "");
    if toks.len() > 2 && ["jr", "sr", "junior", "senior", "ii", "iii", "iv"].contains(&norm(toks[toks.len() - 1]).as_str()) {
        suffix = toks.pop();
    }
    let last = toks.pop().unwrap();
    let mut out = format!("{last}, {}", toks.join(" "));
    if let Some(sf) = suffix {
        out.push(' ');
        out.push_str(sf);
    }
    out
}

fn authors_sort(authors: &[String]) -> String {
    authors.iter().map(|a| author_sort(a)).collect::<Vec<_>>().join(" & ")
}

fn db_now() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.6f+00:00").to_string()
}

/// `YYYY`, `YYYY-MM`, `YYYY-MM-DD` (anything after is ignored) → db timestamp.
fn norm_pubdate(d: &str) -> Option<String> {
    let parts: Vec<&str> = d.trim().split(|c: char| !c.is_ascii_digit()).collect();
    let y = parts.first().filter(|y| y.len() == 4)?;
    let num = |i: usize, max: u32| parts.get(i).and_then(|x| x.parse::<u32>().ok()).filter(|x| (1..=max).contains(x));
    let mo = num(1, 12);
    let dd = if mo.is_some() { num(2, 31).unwrap_or(1) } else { 1 };
    let mo = mo.unwrap_or(1);
    Some(format!("{y}-{mo:02}-{dd:02} 00:00:00+00:00"))
}

// ---------- connection / creation ----------

pub fn open(root: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(root.join("metadata.db"))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", true)?;
    let f = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    conn.create_scalar_function("title_sort", 1, f, |c| {
        Ok(title_sort(&c.get::<Option<String>>(0)?.unwrap_or_default()))
    })?;
    conn.create_scalar_function("author_to_author_sort", 1, f, |c| {
        Ok(author_sort(&c.get::<Option<String>>(0)?.unwrap_or_default()))
    })?;
    conn.create_scalar_function("uuid4", 0, FunctionFlags::SQLITE_UTF8, |_| {
        Ok(uuid::Uuid::new_v4().to_string())
    })?;
    conn.create_scalar_function("books_list_filter", 1, f, |_| Ok(1i64))?;
    conn.create_collation("PYNOCASE", |a, b| a.to_lowercase().cmp(&b.to_lowercase()))?;
    Ok(conn)
}

pub fn ensure(root: &Path) -> R<()> {
    if root.join("metadata.db").exists() {
        return Ok(());
    }
    fs::create_dir_all(root).map_err(s)?;
    let conn = open(root).map_err(s)?;
    conn.execute_batch(include_str!("../resources/metadata_sqlite.sql")).map_err(s)?;
    conn.execute_batch("DELETE FROM library_id;").map_err(s)?;
    conn.execute("INSERT INTO library_id(uuid) VALUES(?)", [uuid::Uuid::new_v4().to_string()]).map_err(s)?;
    Ok(())
}

pub fn library_uuid(conn: &Connection) -> R<String> {
    conn.query_row("SELECT uuid FROM library_id", [], |r| r.get(0)).map_err(s)
}

// ---------- list ----------

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BookRow {
    pub id: i64,
    pub title: String,
    pub authors: Vec<String>,
    pub series: Option<String>,
    pub series_index: f64,
    pub tags: Vec<String>,
    pub formats: Vec<String>,
    pub uuid: String,
    pub cover_path: Option<String>,
    pub last_modified: String,
}

fn split(v: Option<String>) -> Vec<String> {
    v.map(|v| v.split('\u{1f}').map(String::from).collect()).unwrap_or_default()
}

pub fn list_books(conn: &Connection, root: &Path) -> R<Vec<BookRow>> {
    let mut st = conn
        .prepare(
            "SELECT b.id, b.title, b.series_index, b.uuid, b.path, b.has_cover, b.last_modified,
              (SELECT group_concat(name, char(31)) FROM (SELECT a.name FROM books_authors_link l JOIN authors a ON a.id=l.author WHERE l.book=b.id ORDER BY l.id)),
              (SELECT group_concat(name, char(31)) FROM (SELECT t.name FROM books_tags_link l JOIN tags t ON t.id=l.tag WHERE l.book=b.id ORDER BY l.id)),
              (SELECT group_concat(format, char(31)) FROM data WHERE book=b.id),
              (SELECT sr.name FROM books_series_link l JOIN series sr ON sr.id=l.series WHERE l.book=b.id)
             FROM books b ORDER BY b.timestamp DESC, b.id DESC",
        )
        .map_err(s)?;
    let rows = st
        .query_map([], |r| {
            let path: String = r.get(4)?;
            let has_cover: i64 = r.get(5)?;
            Ok(BookRow {
                id: r.get(0)?,
                title: r.get(1)?,
                series_index: r.get(2)?,
                uuid: r.get(3)?,
                cover_path: (has_cover == 1).then(|| root.join(&path).join("cover.jpg").to_string_lossy().into_owned()),
                last_modified: r.get(6)?,
                authors: split(r.get(7)?),
                tags: split(r.get(8)?),
                formats: split(r.get(9)?),
                series: r.get(10)?,
            })
        })
        .map_err(s)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(s)?;
    Ok(rows)
}

/// (title, authors, isbn) for metadata search.
pub fn book_query_info(conn: &Connection, id: i64) -> R<(String, Vec<String>, Option<String>)> {
    let title: String = conn.query_row("SELECT title FROM books WHERE id=?", [id], |r| r.get(0)).map_err(s)?;
    let authors = split(
        conn.query_row(
            "SELECT group_concat(name, char(31)) FROM (SELECT a.name FROM books_authors_link l JOIN authors a ON a.id=l.author WHERE l.book=? ORDER BY l.id)",
            [id],
            |r| r.get(0),
        )
        .map_err(s)?,
    );
    let isbn = conn
        .query_row("SELECT val FROM identifiers WHERE book=? AND type='isbn'", [id], |r| r.get(0))
        .ok();
    Ok((title, authors, isbn))
}

// ---------- add ----------

#[derive(Debug, PartialEq)]
pub enum Added {
    Ok(i64),
    Duplicate,
}

#[derive(Default, Debug)]
pub struct EpubMeta {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<String>,
    pub description: Option<String>,
    pub isbn: Option<String>,
    pub tags: Vec<String>,
    pub cover: Option<Vec<u8>>,
}

fn join_zip_path(base_dir: &str, href: &str) -> String {
    let mut parts: Vec<&str> = base_dir.split('/').filter(|p| !p.is_empty()).collect();
    for p in href.split('/') {
        match p {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn read_zip_entry(z: &mut zip::ZipArchive<fs::File>, name: &str) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    for n in [name.to_string(), name.replace("%20", " ")] {
        if let Ok(mut f) = z.by_name(&n) {
            f.read_to_end(&mut buf).ok()?;
            return Some(buf);
        }
    }
    None
}

fn is_isbn(t: &str) -> Option<String> {
    let t: String = t.trim().trim_start_matches("urn:isbn:").chars().filter(|c| *c != '-' && *c != ' ').collect();
    let ok = match t.len() {
        13 => t.chars().all(|c| c.is_ascii_digit()),
        10 => t[..9].chars().all(|c| c.is_ascii_digit()) && t[9..].chars().all(|c| c.is_ascii_digit() || c == 'X' || c == 'x'),
        _ => false,
    };
    ok.then_some(t)
}

pub fn read_epub(file: &Path) -> Option<EpubMeta> {
    let mut z = zip::ZipArchive::new(fs::File::open(file).ok()?).ok()?;
    let container = String::from_utf8(read_zip_entry(&mut z, "META-INF/container.xml")?).ok()?;
    let cdoc = roxmltree::Document::parse(&container).ok()?;
    let opf_path = cdoc
        .descendants()
        .find(|n| n.has_tag_name("rootfile"))?
        .attribute("full-path")?
        .to_string();
    let opf = String::from_utf8(read_zip_entry(&mut z, &opf_path)?).ok()?;
    let doc = roxmltree::Document::parse(&opf).ok()?;
    let els = |name: &'static str| doc.descendants().filter(move |n| n.is_element() && n.tag_name().name() == name);
    let text = |n: roxmltree::Node| {
        let t = n.text().unwrap_or("").trim().to_string();
        (!t.is_empty()).then_some(t)
    };
    let mut m = EpubMeta {
        title: els("title").find_map(text),
        authors: els("creator").filter_map(text).collect(),
        publisher: els("publisher").find_map(text),
        pubdate: els("date").find_map(text),
        description: els("description").find_map(text),
        tags: els("subject").filter_map(text).collect(),
        ..Default::default()
    };
    m.isbn = els("identifier").find_map(|n| {
        let by_scheme = n.attributes().any(|a| a.name() == "scheme" && a.value().eq_ignore_ascii_case("isbn"));
        let t = n.text().unwrap_or("");
        is_isbn(t).or_else(|| by_scheme.then(|| t.trim().to_string()))
    });
    // cover
    let cover_id = els("meta")
        .find(|n| n.attribute("name") == Some("cover"))
        .and_then(|n| n.attribute("content"));
    let item = els("item").find(|n| cover_id.is_some() && n.attribute("id") == cover_id).or_else(|| {
        els("item").find(|n| n.attribute("properties").is_some_and(|p| p.split_whitespace().any(|x| x == "cover-image")))
    });
    if let Some(href) = item.and_then(|n| n.attribute("href")) {
        let dir = opf_path.rsplit_once('/').map(|x| x.0).unwrap_or("");
        m.cover = read_zip_entry(&mut z, &join_zip_path(dir, href));
    }
    Some(m)
}

/// Write `dir/cover.jpg` (JPEG kept as is, anything else re-encoded at q90).
pub fn save_cover(dir: &Path, bytes: &[u8]) -> R<()> {
    let fmt = image::guess_format(bytes).map_err(|_| "capa inválida".to_string())?;
    let out = dir.join("cover.jpg");
    if fmt == image::ImageFormat::Jpeg {
        return fs::write(out, bytes).map_err(s);
    }
    let img = image::load_from_memory(bytes).map_err(|_| "capa inválida".to_string())?.to_rgb8();
    let mut f = fs::File::create(out).map_err(s)?;
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut f, 90)
        .encode_image(&img)
        .map_err(s)
}

struct NewBook {
    title: String,
    authors: Vec<String>,
    publisher: Option<String>,
    pubdate: Option<String>,
    description: Option<String>,
    isbn: Option<String>,
    tags: Vec<String>,
    cover: Option<Vec<u8>>,
}

fn from_filename(file: &Path) -> NewBook {
    let stem = file.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
    let (title, author) = match stem.split_once(" - ") {
        Some((t, a)) => (t.trim().to_string(), a.trim().to_string()),
        None => (stem.clone(), UNKNOWN_AUTHOR.to_string()),
    };
    NewBook {
        title,
        authors: vec![if author.is_empty() { UNKNOWN_AUTHOR.into() } else { author }],
        publisher: None,
        pubdate: None,
        description: None,
        isbn: None,
        tags: vec![],
        cover: None,
    }
}

fn item_id(tx: &Connection, table: &str, name: &str, sort: Option<&str>) -> R<i64> {
    if let Ok(id) = tx.query_row(&format!("SELECT id FROM {table} WHERE name=?"), [name], |r| r.get(0)) {
        return Ok(id);
    }
    match sort {
        Some(so) => tx.execute(&format!("INSERT INTO {table}(name, sort) VALUES(?,?)"), params![name, so]),
        None => tx.execute(&format!("INSERT INTO {table}(name) VALUES(?)"), [name]),
    }
    .map_err(s)?;
    Ok(tx.last_insert_rowid())
}

/// Replace authors; returns the new `author_sort`.
fn set_authors(tx: &Connection, id: i64, authors: &[String]) -> R<String> {
    tx.execute("DELETE FROM books_authors_link WHERE book=?", [id]).map_err(s)?;
    for a in authors {
        let a_id = item_id(tx, "authors", a, Some(&author_sort(a)))?;
        tx.execute("INSERT OR IGNORE INTO books_authors_link(book, author) VALUES(?,?)", [id, a_id]).map_err(s)?;
    }
    Ok(authors_sort(authors))
}

fn set_tags(tx: &Connection, id: i64, tags: &[String]) -> R<()> {
    tx.execute("DELETE FROM books_tags_link WHERE book=?", [id]).map_err(s)?;
    for t in tags {
        let t_id = item_id(tx, "tags", t, None)?;
        tx.execute("INSERT OR IGNORE INTO books_tags_link(book, tag) VALUES(?,?)", [id, t_id]).map_err(s)?;
    }
    Ok(())
}

fn set_publisher(tx: &Connection, id: i64, p: &str) -> R<()> {
    tx.execute("DELETE FROM books_publishers_link WHERE book=?", [id]).map_err(s)?;
    let p_id = item_id(tx, "publishers", p, Some(p))?;
    tx.execute("INSERT INTO books_publishers_link(book, publisher) VALUES(?,?)", [id, p_id]).map_err(s)?;
    Ok(())
}

fn set_comment(tx: &Connection, id: i64, text: &str) -> R<()> {
    tx.execute(
        "INSERT INTO comments(book,text) VALUES(?,?) ON CONFLICT(book) DO UPDATE SET text=excluded.text",
        params![id, text],
    )
    .map_err(s)?;
    Ok(())
}

fn set_identifier(tx: &Connection, id: i64, typ: &str, val: &str) -> R<()> {
    tx.execute("INSERT OR REPLACE INTO identifiers(book,type,val) VALUES(?,?,?)", params![id, typ, val]).map_err(s)?;
    Ok(())
}

pub fn add_book(conn: &Connection, root: &Path, file: &Path) -> R<Added> {
    let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !ADD_EXTENSIONS.contains(&ext.as_str()) {
        return Err("formato não suportado".into());
    }
    let mut nb = from_filename(file);
    if ext == "epub" {
        if let Some(m) = read_epub(file) {
            if let Some(t) = m.title {
                nb.title = t;
            }
            if !m.authors.is_empty() {
                nb.authors = m.authors;
            }
            nb.publisher = m.publisher;
            nb.pubdate = m.pubdate.as_deref().and_then(norm_pubdate);
            nb.description = m.description;
            nb.isbn = m.isbn;
            nb.tags = m.tags;
            nb.cover = m.cover;
        }
    }
    let dup = conn
        .query_row(
            "SELECT 1 FROM books b JOIN books_authors_link l ON l.book=b.id JOIN authors a ON a.id=l.author
             WHERE lower(b.title)=lower(?1) AND lower(a.name)=lower(?2)",
            params![nb.title, nb.authors[0]],
            |_| Ok(()),
        )
        .is_ok();
    if dup {
        return Ok(Added::Duplicate);
    }
    let tx = conn.unchecked_transaction().map_err(s)?;
    let mut created = None;
    let r = insert(&tx, root, file, &ext, &nb, &mut created).and_then(|id| tx.commit().map(|_| id).map_err(s));
    match r {
        Ok(id) => Ok(Added::Ok(id)),
        Err(e) => {
            if let Some(d) = created {
                let _ = fs::remove_dir_all(&d);
                if let Some(p) = d.parent() {
                    let _ = fs::remove_dir(p);
                }
            }
            Err(e)
        }
    }
}

fn insert(tx: &Connection, root: &Path, file: &Path, ext: &str, nb: &NewBook, created: &mut Option<PathBuf>) -> R<i64> {
    let author_sort = authors_sort(&nb.authors);
    tx.execute("INSERT INTO books(title, series_index, author_sort) VALUES(?,1.0,?)", params![nb.title, author_sort]).map_err(s)?;
    let id = tx.last_insert_rowid();
    let first = &nb.authors[0];
    let path = construct_path_name(id, &nb.title, first);
    let name = construct_file_name(id, &nb.title, first, ext.len() + 1);
    let now = db_now();
    tx.execute(
        "UPDATE books SET path=?, timestamp=?, pubdate=?, last_modified=?, has_cover=0 WHERE id=?",
        params![path, now, nb.pubdate.as_deref().unwrap_or(UNDEFINED_DATE), now, id],
    )
    .map_err(s)?;
    set_authors(tx, id, &nb.authors)?;
    set_tags(tx, id, &nb.tags)?;
    if let Some(p) = &nb.publisher {
        set_publisher(tx, id, p)?;
    }
    if let Some(d) = &nb.description {
        set_comment(tx, id, d)?;
    }
    if let Some(i) = &nb.isbn {
        set_identifier(tx, id, "isbn", i)?;
    }
    let size = fs::metadata(file).map_err(s)?.len();
    tx.execute(
        "INSERT INTO data(book, format, uncompressed_size, name) VALUES(?, UPPER(?), ?, ?)",
        params![id, ext, size as i64, name],
    )
    .map_err(s)?;
    tx.execute("INSERT OR IGNORE INTO metadata_dirtied(book) VALUES(?)", [id]).map_err(s)?;

    let dir = root.join(&path);
    fs::create_dir_all(&dir).map_err(s)?;
    *created = Some(dir.clone());
    fs::copy(file, dir.join(format!("{name}.{ext}"))).map_err(s)?;
    if let Some(c) = &nb.cover {
        // a broken embedded cover must not block adding the book
        if save_cover(&dir, c).is_ok() {
            tx.execute("UPDATE books SET has_cover=1 WHERE id=?", [id]).map_err(s)?;
        }
    }
    Ok(id)
}


// ---------- send ----------

#[derive(Debug)]
pub struct SendBook {
    pub id: i64,
    pub title: String,
    pub title_sort: String,
    pub authors: Vec<String>,
    pub author_sort: String,
    pub tags: Vec<String>,
    pub series: Option<String>,
    pub series_index: f64,
    pub uuid: String,
    pub last_modified: String,
    pub file: PathBuf,
    pub ext: String,
    pub size: u64,
}

pub fn book_for_send(conn: &Connection, root: &Path, id: i64, accepted: &[String]) -> R<SendBook> {
    let (title, authors, _) = book_query_info(conn, id)?;
    let (sort, author_sort, series_index, uuid, path, lm): (String, String, f64, String, String, String) = conn
        .query_row(
            "SELECT sort, author_sort, series_index, uuid, path, last_modified FROM books WHERE id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .map_err(s)?;
    let formats: Vec<(String, String, i64)> = conn
        .prepare("SELECT format, name, uncompressed_size FROM data WHERE book=?")
        .map_err(s)?
        .query_map([id], |r| Ok((r.get::<_, String>(0)?.to_lowercase(), r.get(1)?, r.get(2)?)))
        .map_err(s)?
        .collect::<Result<_, _>>()
        .map_err(s)?;
    let (ext, name, size) = accepted
        .iter()
        .find_map(|a| formats.iter().find(|f| &f.0 == a))
        .cloned()
        .ok_or("nenhum formato aceito pelo KOReader")?;
    let tags = split(
        conn.query_row(
            "SELECT group_concat(name, char(31)) FROM (SELECT t.name FROM books_tags_link l JOIN tags t ON t.id=l.tag WHERE l.book=? ORDER BY l.id)",
            [id],
            |r| r.get(0),
        )
        .map_err(s)?,
    );
    let series = conn
        .query_row("SELECT sr.name FROM books_series_link l JOIN series sr ON sr.id=l.series WHERE l.book=?", [id], |r| r.get(0))
        .ok();
    let file = root.join(&path).join(format!("{name}.{ext}"));
    let size = if size > 0 { size as u64 } else { fs::metadata(&file).map(|m| m.len()).unwrap_or(0) };
    Ok(SendBook {
        id,
        title,
        title_sort: sort,
        authors,
        author_sort,
        tags,
        series,
        series_index,
        uuid,
        last_modified: lm.replacen(' ', "T", 1),
        file,
        ext,
        size,
    })
}

// ---------- apply downloaded metadata ----------

pub fn apply_metadata(conn: &Connection, root: &Path, id: i64, c: &Candidate, meta: bool, cover: Option<Vec<u8>>) -> R<()> {
    let tx = conn.unchecked_transaction().map_err(s)?;
    let path: String = tx.query_row("SELECT path FROM books WHERE id=?", [id], |r| r.get(0)).map_err(s)?;
    if meta {
        if !c.title.trim().is_empty() {
            tx.execute("UPDATE books SET title=? WHERE id=?", params![c.title.trim(), id]).map_err(s)?;
        }
        if !c.authors.is_empty() {
            let a_sort = set_authors(&tx, id, &c.authors)?;
            tx.execute("UPDATE books SET author_sort=? WHERE id=?", params![a_sort, id]).map_err(s)?;
        }
        if let Some(p) = c.publisher.as_deref().filter(|p| !p.is_empty()) {
            set_publisher(&tx, id, p)?;
        }
        if let Some(d) = c.pubdate.as_deref().and_then(norm_pubdate) {
            tx.execute("UPDATE books SET pubdate=? WHERE id=?", params![d, id]).map_err(s)?;
        }
        if let Some(d) = c.description.as_deref().filter(|d| !d.is_empty()) {
            set_comment(&tx, id, d)?;
        }
        if !c.tags.is_empty() {
            set_tags(&tx, id, &c.tags)?;
        }
        if let Some(i) = c.isbn.as_deref().filter(|i| !i.is_empty()) {
            set_identifier(&tx, id, "isbn", i)?;
        }
        if let Some(g) = c.google_id.as_deref().filter(|g| !g.is_empty()) {
            set_identifier(&tx, id, "google", g)?;
        }
    }
    if let Some(bytes) = cover {
        save_cover(&root.join(&path), &bytes)?;
        tx.execute("UPDATE books SET has_cover=1 WHERE id=?", [id]).map_err(s)?;
    }
    tx.execute("UPDATE books SET last_modified=? WHERE id=?", params![db_now(), id]).map_err(s)?;
    tx.execute("INSERT OR IGNORE INTO metadata_dirtied(book) VALUES(?)", [id]).map_err(s)?;
    // ponytail: no folder rename on title/author change; calibre fixes names on its next edit
    tx.commit().map_err(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    #[test]
    fn naming() {
        assert_eq!(title_sort("The Hobbit"), "Hobbit, The");
        assert_eq!(title_sort("O Alienista"), "Alienista, O");
        assert_eq!(title_sort("Dom Casmurro"), "Dom Casmurro");
        assert_eq!(construct_path_name(12, "Dom Casmurro", "Machado de Assis"), "Machado de Assis/Dom Casmurro (12)");
        assert_eq!(ascii_filename("José de Alencar: Iracema?"), "Jose de Alencar_ Iracema_");
        assert_eq!(author_sort("Martin Luther King Jr."), "King, Martin Luther Jr.");
        assert_eq!(author_sort("Machado de Assis"), "Assis, Machado de");
        assert_eq!(norm_pubdate("2008-05").unwrap(), "2008-05-01 00:00:00+00:00");
        assert_eq!(norm_pubdate("1899").unwrap(), "1899-01-01 00:00:00+00:00");
    }

    fn make_epub(path: &Path) {
        let mut png = Cursor::new(Vec::new());
        image::RgbImage::new(4, 4).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let mut z = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("META-INF/container.xml", o).unwrap();
        z.write_all(br#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
        z.start_file("OEBPS/content.opf", o).unwrap();
        z.write_all(br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf"><dc:title>O Alienista</dc:title><dc:creator>Machado de Assis</dc:creator><dc:identifier opf:scheme="ISBN">9788535910667</dc:identifier><meta name="cover" content="cv"/></metadata><manifest><item id="cv" href="img/cover.png" media-type="image/png"/></manifest></package>"#).unwrap();
        z.start_file("OEBPS/img/cover.png", o).unwrap();
        z.write_all(png.get_ref()).unwrap();
        z.finish().unwrap();
    }

    #[test]
    fn create_and_add() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        ensure(&root).unwrap();
        let conn = open(&root).unwrap();
        assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 28);
        assert_eq!(conn.query_row("SELECT count(*) FROM library_id", [], |r| r.get::<_, i64>(0)).unwrap(), 1);

        let epub = dir.path().join("x.epub");
        make_epub(&epub);
        assert_eq!(add_book(&conn, &root, &epub).unwrap(), Added::Ok(1));
        let sort: String = conn.query_row("SELECT sort FROM books WHERE id=1", [], |r| r.get(0)).unwrap();
        assert_eq!(sort, "Alienista, O");
        let uuid: Option<String> = conn.query_row("SELECT uuid FROM books WHERE id=1", [], |r| r.get(0)).unwrap();
        assert!(uuid.is_some());
        let fmt: String = conn.query_row("SELECT format FROM data WHERE book=1", [], |r| r.get(0)).unwrap();
        assert_eq!(fmt, "EPUB");
        let bdir = root.join("Machado de Assis/O Alienista (1)");
        assert!(bdir.join("O Alienista - Machado de Assis.epub").exists());
        let cover = fs::read(bdir.join("cover.jpg")).unwrap();
        assert_eq!(image::guess_format(&cover).unwrap(), image::ImageFormat::Jpeg);
        assert_eq!(conn.query_row("SELECT count(*) FROM metadata_dirtied WHERE book=1", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        let isbn: String = conn.query_row("SELECT val FROM identifiers WHERE book=1 AND type='isbn'", [], |r| r.get(0)).unwrap();
        assert_eq!(isbn, "9788535910667");
        assert_eq!(add_book(&conn, &root, &epub).unwrap(), Added::Duplicate);

        let rows = list_books(&conn, &root).unwrap();
        assert_eq!(rows[0].authors, vec!["Machado de Assis"]);
        assert!(rows[0].cover_path.is_some());
        let sb = book_for_send(&conn, &root, 1, &["pdf".into(), "epub".into()]).unwrap();
        assert_eq!(sb.ext, "epub");
        assert!(sb.file.exists());
        assert!(book_for_send(&conn, &root, 1, &["pdf".into()]).is_err());
    }
}

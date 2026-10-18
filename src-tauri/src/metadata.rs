//! Metadata + cover lookup: Google Books and Open Library.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{io::Read, time::Duration};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub source: String, // "google" | "openlibrary"
    pub title: String,
    pub authors: Vec<String>,
    pub publisher: Option<String>,
    pub pubdate: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub isbn: Option<String>,
    pub google_id: Option<String>,
    pub cover_url: Option<String>,
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub candidates: Vec<Candidate>,
    pub errors: Vec<String>,
}

// Google's "image not available" placeholders (from calibre's google.py)
const GOOGLE_PLACEHOLDERS: [&str; 2] = ["0de4383ebad0adad5eeb8975cd796657", "a64fa89d7ebc97075c1d363fc5fea71f"];

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).user_agent("CalibreLite/0.1").build()
}

fn get_json(url: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    let mut r = agent().get(url);
    for (k, v) in query {
        r = r.query(k, v);
    }
    r.call().map_err(|e| e.to_string())?.into_json().map_err(|e| e.to_string())
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn query_pair(title: &str, authors: &[String], isbn: Option<&str>, google: bool) -> Vec<(&'static str, String)> {
    let author = authors.first().filter(|a| !a.is_empty() && *a != "Desconhecido");
    if let Some(i) = isbn {
        return vec![if google { ("q", format!("isbn:{i}")) } else { ("isbn", i.to_string()) }];
    }
    if google {
        let mut q = format!("intitle:{title}");
        if let Some(a) = author {
            q.push_str(&format!(" inauthor:{a}"));
        }
        vec![("q", q)]
    } else {
        let mut v = vec![("title", title.to_string())];
        if let Some(a) = author {
            v.push(("author", a.clone()));
        }
        v
    }
}

fn google(title: &str, authors: &[String], isbn: Option<&str>) -> Result<Vec<Candidate>, String> {
    let mut q = query_pair(title, authors, isbn, true);
    q.push(("maxResults", "10".into()));
    let q: Vec<(&str, &str)> = q.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let v = get_json("https://www.googleapis.com/books/v1/volumes", &q)?;
    let items = v["items"].as_array().cloned().unwrap_or_default();
    Ok(items
        .iter()
        .map(|it| {
            let vi = &it["volumeInfo"];
            let ids = vi["industryIdentifiers"].as_array().cloned().unwrap_or_default();
            let pick = |t: &str| ids.iter().find(|i| i["type"] == t).and_then(|i| i["identifier"].as_str()).map(String::from);
            let id = it["id"].as_str().map(String::from);
            Candidate {
                source: "google".into(),
                title: vi["title"].as_str().unwrap_or_default().into(),
                authors: strings(&vi["authors"]),
                publisher: vi["publisher"].as_str().map(String::from),
                pubdate: vi["publishedDate"].as_str().map(String::from),
                description: vi["description"].as_str().map(String::from),
                tags: strings(&vi["categories"]),
                isbn: pick("ISBN_13").or_else(|| pick("ISBN_10")),
                cover_url: (!vi["imageLinks"].is_null())
                    .then(|| id.as_ref().map(|i| format!("https://books.google.com/books?id={i}&printsec=frontcover&img=1")))
                    .flatten(),
                google_id: id,
            }
        })
        .filter(|c| !c.title.is_empty())
        .collect())
}

fn open_library(title: &str, authors: &[String], isbn: Option<&str>) -> Result<Vec<Candidate>, String> {
    let mut q = query_pair(title, authors, isbn, false);
    q.push(("limit", "10".into()));
    q.push(("fields", "key,title,author_name,first_publish_year,publisher,isbn,cover_i,subject".into()));
    let q: Vec<(&str, &str)> = q.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let v = get_json("https://openlibrary.org/search.json", &q)?;
    let docs = v["docs"].as_array().cloned().unwrap_or_default();
    Ok(docs
        .iter()
        .map(|d| {
            let isbns = strings(&d["isbn"]);
            Candidate {
                source: "openlibrary".into(),
                title: d["title"].as_str().unwrap_or_default().into(),
                authors: strings(&d["author_name"]),
                publisher: strings(&d["publisher"]).into_iter().next(),
                pubdate: d["first_publish_year"].as_i64().map(|y| y.to_string()),
                description: None,
                tags: strings(&d["subject"]).into_iter().take(5).collect(),
                isbn: isbns.iter().find(|i| i.len() == 13).or_else(|| isbns.first()).cloned(),
                google_id: None,
                cover_url: d["cover_i"].as_i64().map(|c| format!("https://covers.openlibrary.org/b/id/{c}-L.jpg")),
            }
        })
        .filter(|c| !c.title.is_empty())
        .collect())
}

pub fn search(title: &str, authors: &[String], isbn: Option<&str>) -> SearchResult {
    let mut out = SearchResult::default();
    for (label, r) in [("Google Books", google(title, authors, isbn)), ("Open Library", open_library(title, authors, isbn))] {
        match r {
            Ok(c) => out.candidates.extend(c),
            Err(e) => out.errors.push(format!("{label}: {e}")),
        }
    }
    out
}

pub fn download_cover(c: &Candidate) -> Result<Vec<u8>, String> {
    let url = c.cover_url.as_deref().ok_or("capa não disponível")?;
    let resp = agent().get(url).call().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    resp.into_reader().take(10 * 1024 * 1024).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let md5 = format!("{:x}", md5::compute(&bytes));
    if GOOGLE_PLACEHOLDERS.contains(&md5.as_str()) || image::guess_format(&bytes).is_err() {
        return Err("capa não disponível".into());
    }
    Ok(bytes)
}

fn norm(s: &str) -> String {
    deunicode::deunicode(s).to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Word set of a person name, accent/case/order-insensitive ("Assis, Machado de" == "Machado de Assis").
fn name_words(s: &str) -> Vec<String> {
    let mut w: Vec<String> = deunicode::deunicode(s)
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|x| !x.is_empty())
        .map(String::from)
        .collect();
    w.sort();
    w
}

/// Best candidate for a book, or None. Never returns a candidate whose title differs, or whose
/// authors don't include the book's known author (wrong metadata is worse than none).
/// An exact ISBN match wins over everything else.
pub fn best<'a>(cands: &'a [Candidate], title: &str, authors: &[String], isbn: Option<&str>) -> Option<&'a Candidate> {
    let t = norm(title);
    let author = authors.first().filter(|a| *a != "Desconhecido").map(|a| name_words(a));
    let digits = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    cands
        .iter()
        .filter(|c| norm(&c.title) == t)
        .filter(|c| author.as_ref().is_none_or(|a| c.authors.iter().any(|x| name_words(x) == *a)))
        .max_by_key(|c| {
            let isbn_ok = isbn.zip(c.isbn.as_deref()).is_some_and(|(a, b)| digits(a) == digits(b));
            ((isbn_ok as u8) * 16) + (c.cover_url.is_some() as u8 * 4) + (c.description.is_some() as u8 * 2) + (c.isbn.is_some() as u8)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(title: &str, authors: &[&str], cover: bool, desc: bool, isbn: Option<&str>) -> Candidate {
        Candidate {
            title: title.into(),
            authors: authors.iter().map(|s| s.to_string()).collect(),
            cover_url: cover.then(|| "u".into()),
            description: desc.then(|| "d".into()),
            isbn: isbn.map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn rejects_other_author_even_if_richer() {
        let me = vec!["Machado de Assis".to_string()];
        let cands = [
            c("Dom Casmurro", &["Machado de Assis"], false, false, None),
            c("Dom Casmurro", &["Outro Autor"], true, true, Some("9780000000002")),
        ];
        assert_eq!(best(&cands, "Dom Casmurro", &me, None).unwrap().authors[0], "Machado de Assis");
        // later wrong-author candidate alone → nothing
        assert!(best(&cands[1..], "Dom Casmurro", &me, None).is_none());
    }

    #[test]
    fn author_order_and_accents_ok_unknown_author_allowed() {
        let cands = [c("Dom Casmurro", &["Assis, Machado de"], false, false, None)];
        assert!(best(&cands, "dom casmurro", &["Machado de Assís".into()], None).is_some());
        assert!(best(&cands, "Dom Casmurro", &["Desconhecido".into()], None).is_some());
    }

    #[test]
    fn isbn_wins_and_title_must_match() {
        let me = vec!["A B".to_string()];
        let cands = [
            c("X", &["A B"], true, true, Some("111")),
            c("X", &["A B"], false, false, Some("978-85-359-1066-7")),
            c("Y", &["A B"], true, true, None),
        ];
        assert_eq!(best(&cands, "X", &me, Some("9788535910667")).unwrap().isbn.as_deref(), Some("978-85-359-1066-7"));
        assert!(best(&cands, "Z", &me, None).is_none());
    }
}

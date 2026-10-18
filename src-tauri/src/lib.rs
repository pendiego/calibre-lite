mod device;
mod library;
mod metadata;

use device::{Cmd, DeviceEvent, DeviceStatus, SendProgress};
use metadata::{Candidate, SearchResult};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{Emitter, Manager, State};

struct AppState {
    root: PathBuf,
    device: device::Handle,
}

#[derive(Serialize)]
struct Skipped {
    file: String,
    reason: String,
}

#[derive(Serialize)]
struct AddReport {
    added: usize,
    ids: Vec<i64>,
    skipped: Vec<Skipped>,
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn library_path(st: State<AppState>) -> String {
    st.root.to_string_lossy().into_owned()
}

#[tauri::command]
async fn list_books(st: State<'_, AppState>) -> Result<Vec<library::BookRow>, String> {
    let root = st.root.clone();
    blocking(move || library::list_books(&library::open(&root).map_err(|e| e.to_string())?, &root)).await
}

#[tauri::command]
async fn add_books(st: State<'_, AppState>, paths: Vec<String>) -> Result<AddReport, String> {
    let root = st.root.clone();
    blocking(move || {
        let conn = library::open(&root).map_err(|e| e.to_string())?;
        let mut report = AddReport { added: 0, ids: vec![], skipped: vec![] };
        for p in paths {
            let name = std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(p.clone());
            match library::add_book(&conn, &root, std::path::Path::new(&p)) {
                Ok(library::Added::Ok(id)) => {
                    report.added += 1;
                    report.ids.push(id);
                }
                Ok(library::Added::Duplicate) => report.skipped.push(Skipped { file: name, reason: "já está na biblioteca".into() }),
                Err(e) => report.skipped.push(Skipped { file: name, reason: e }),
            }
        }
        Ok(report)
    })
    .await
}

#[tauri::command]
fn device_status(st: State<AppState>) -> DeviceStatus {
    device::status(&st.device.shared.lock().unwrap())
}

#[tauri::command]
async fn send_books(app: tauri::AppHandle, st: State<'_, AppState>, ids: Vec<i64>) -> Result<(), String> {
    let (accepted, lens) = {
        let g = st.device.shared.lock().unwrap();
        if !g.connected {
            return Err("KOReader não conectado".into());
        }
        (g.accepted.clone(), g.ext_path_lengths.clone())
    };
    let root = st.root.clone();
    let (items, errors) = blocking(move || {
        let conn = library::open(&root).map_err(|e| e.to_string())?;
        let (mut items, mut errors) = (vec![], vec![]);
        for id in ids {
            match library::book_for_send(&conn, &root, id, &accepted) {
                Ok(b) => items.push(device::send_item(&b, lens.get(&b.ext).copied().unwrap_or(40))),
                Err(e) => errors.push((id, conn_title(&conn, id), e)),
            }
        }
        Ok((items, errors))
    })
    .await?;
    let total = items.len() + errors.len();
    for (i, (_, title, e)) in errors.into_iter().enumerate() {
        let _ = app.emit("send-progress", SendProgress { current: i + 1, total, title, error: Some(e), done: items.is_empty() && i + 1 == total });
    }
    if !items.is_empty() {
        st.device.tx.send(Cmd::Send(items)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn conn_title(conn: &rusqlite::Connection, id: i64) -> String {
    conn.query_row("SELECT title FROM books WHERE id=?", [id], |r| r.get(0)).unwrap_or_else(|_| id.to_string())
}

#[tauri::command]
async fn search_metadata(st: State<'_, AppState>, id: i64) -> Result<SearchResult, String> {
    let root = st.root.clone();
    blocking(move || {
        let (title, authors, isbn) = library::book_query_info(&library::open(&root).map_err(|e| e.to_string())?, id)?;
        Ok(metadata::search(&title, &authors, isbn.as_deref()))
    })
    .await
}

#[tauri::command]
async fn apply_metadata(st: State<'_, AppState>, id: i64, candidate: Candidate, meta: bool, cover: bool) -> Result<(), String> {
    let root = st.root.clone();
    blocking(move || {
        let cover = if cover && candidate.cover_url.is_some() { Some(metadata::download_cover(&candidate)?) } else { None };
        library::apply_metadata(&library::open(&root).map_err(|e| e.to_string())?, &root, id, &candidate, meta, cover)
    })
    .await
}

#[derive(Serialize)]
struct AutoReport {
    updated: usize,
    failed: Vec<String>,
}

/// Search both sources for each book, apply the best title match (metadata + cover).
#[tauri::command]
async fn auto_metadata(st: State<'_, AppState>, ids: Vec<i64>) -> Result<AutoReport, String> {
    let root = st.root.clone();
    blocking(move || {
        let conn = library::open(&root).map_err(|e| e.to_string())?;
        let mut rep = AutoReport { updated: 0, failed: vec![] };
        for id in ids {
            let (title, authors, isbn) = library::book_query_info(&conn, id)?;
            let found = metadata::search(&title, &authors, isbn.as_deref());
            let Some(c) = metadata::best(&found.candidates, &title, &authors, isbn.as_deref()) else {
                let why = found.errors.first().cloned().unwrap_or_else(|| "nada encontrado".into());
                rep.failed.push(format!("{title}: {why}"));
                continue;
            };
            let cover = metadata::download_cover(c).ok();
            match library::apply_metadata(&conn, &root, id, c, true, cover) {
                Ok(()) => rep.updated += 1,
                Err(e) => rep.failed.push(format!("{title}: {e}")),
            }
        }
        Ok(rep)
    })
    .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let root = app.path().document_dir()?.join("Calibre Library");
            library::ensure(&root)?;
            app.asset_protocol_scope().allow_directory(&root, true)?;
            let uuid = library::library_uuid(&library::open(&root)?)?;
            let handle = app.handle().clone();
            let emit: std::sync::Arc<dyn Fn(DeviceEvent) + Send + Sync> = std::sync::Arc::new(move |e| {
                let _ = match e {
                    DeviceEvent::Status(s) => handle.emit("device-status", s),
                    DeviceEvent::Progress(p) => handle.emit("send-progress", p),
                };
            });
            let device = device::start("0.0.0.0:9090", &device::BROADCAST_PORTS, "Calibre Library".into(), uuid, emit)?;
            app.manage(AppState { root, device });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![library_path, list_books, add_books, device_status, send_books, search_metadata, apply_metadata, auto_metadata])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

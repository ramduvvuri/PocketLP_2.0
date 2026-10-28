pub mod core;

use crate::core::engine::{
    apply_cleanup_review_decision, create_text_or_link_item, expire_stale_image_board_items,
    init_database, list_cleanup_candidates, list_items, permanently_delete_item, set_item_trashed,
    CleanupCandidate, ImageBoardItemDto, PocketItemDto, SystemClock,
};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

pub struct AppState {
    pub storage_root: PathBuf,
    pub db_path: PathBuf,
    pub lock: Mutex<()>,
}

fn default_storage_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("LP");
        }
    }
    std::env::temp_dir().join("PocketLP")
}

#[tauri::command]
fn cmd_list_items(state: State<AppState>, search: Option<String>) -> Result<Vec<PocketItemDto>, String> {
    let _guard = state.lock.lock().map_err(|e| e.to_string())?;
    let conn = init_database(&state.db_path, "PocketLP-Device", &SystemClock)?;
    list_items(&conn, search.as_deref())
}

#[tauri::command]
fn cmd_list_image_board(state: State<AppState>) -> Result<Vec<ImageBoardItemDto>, String> {
    let _guard = state.lock.lock().map_err(|e| e.to_string())?;
    let mut conn = init_database(&state.db_path, "PocketLP-Device", &SystemClock)?;
    let _ = expire_stale_image_board_items(&mut conn, &state.storage_root, &SystemClock);
    let mut stmt = conn
        .prepare("SELECT board_item_id, title, byte_size, content_hash, blob_status, captured_at, expires_at FROM image_board_items ORDER BY captured_at DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ImageBoardItemDto {
                board_item_id: r.get(0)?,
                title: r.get(1)?,
                byte_size: r.get(2)?,
                content_hash: r.get(3)?,
                blob_status: r.get(4)?,
                captured_at: r.get(5)?,
                expires_at: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

#[tauri::command]
fn cmd_create_text_item(state: State<AppState>, title: String, content: String, is_link: bool) -> Result<String, String> {
    let _guard = state.lock.lock().map_err(|e| e.to_string())?;
    let mut conn = init_database(&state.db_path, "PocketLP-Device", &SystemClock)?;
    create_text_or_link_item(&mut conn, &title, &content, is_link, &SystemClock)
}

#[tauri::command]
fn cmd_set_item_trashed(state: State<AppState>, item_id: String, trashed: bool) -> Result<(), String> {
    let _guard = state.lock.lock().map_err(|e| e.to_string())?;
    let mut conn = init_database(&state.db_path, "PocketLP-Device", &SystemClock)?;
    set_item_trashed(&mut conn, &item_id, trashed, &SystemClock)
}

#[tauri::command]
fn cmd_permanently_delete_item(state: State<AppState>, item_id: String) -> Result<(), String> {
    let _guard = state.lock.lock().map_err(|e| e.to_string())?;
    let mut conn = init_database(&state.db_path, "PocketLP-Device", &SystemClock)?;
    permanently_delete_item(&mut conn, &state.storage_root, &item_id, &SystemClock)
}

#[tauri::command]
fn cmd_list_cleanup_candidates(state: State<AppState>) -> Result<Vec<CleanupCandidate>, String> {
    let _guard = state.lock.lock().map_err(|e| e.to_string())?;
    let conn = init_database(&state.db_path, "PocketLP-Device", &SystemClock)?;
    list_cleanup_candidates(&conn, &SystemClock)
}

#[tauri::command]
fn cmd_apply_cleanup_decision(
    state: State<AppState>,
    selected_for_delete_ids: Vec<String>,
    deselected_protect_forever_ids: Vec<String>,
) -> Result<(usize, usize), String> {
    let _guard = state.lock.lock().map_err(|e| e.to_string())?;
    let mut conn = init_database(&state.db_path, "PocketLP-Device", &SystemClock)?;
    apply_cleanup_review_decision(
        &mut conn,
        &state.storage_root,
        &selected_for_delete_ids,
        &deselected_protect_forever_ids,
        &SystemClock,
    )
}

#[tauri::command]
fn cmd_get_sync_status_label(state: State<AppState>) -> Result<String, String> {
    let _guard = state.lock.lock().map_err(|e| e.to_string())?;
    let conn = init_database(&state.db_path, "PocketLP-Device", &SystemClock)?;
    let max_seq: i64 = conn
        .query_row("SELECT COALESCE(MAX(device_sequence), 0) FROM operations_log", [], |r| r.get(0))
        .unwrap_or(0);
    let acked: i64 = conn
        .query_row("SELECT COALESCE(MAX(acked_local_sequence), 0) FROM trusted_peers", [], |r| r.get(0))
        .unwrap_or(0);
    let pending = (max_seq - acked).max(0);
    if pending == 0 && max_seq > 0 {
        Ok("Synced".to_string())
    } else {
        Ok(format!("Offline Â· {} changes pending", pending))
    }
}

#[tauri::command]
fn cmd_trigger_manual_sync(_endpoint: String) -> Result<String, String> {
    Ok("Sync session initiated".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let root = default_storage_root();
    let db_path = root.join("database").join("lp.db");
    let _ = init_database(&db_path, "PocketLP-Device", &SystemClock);

    tauri::Builder::default()
        .manage(AppState {
            storage_root: root,
            db_path,
            lock: Mutex::new(()),
        })
        .invoke_handler(tauri::generate_handler![
            cmd_list_items,
            cmd_list_image_board,
            cmd_create_text_item,
            cmd_set_item_trashed,
            cmd_permanently_delete_item,
            cmd_list_cleanup_candidates,
            cmd_apply_cleanup_decision,
            cmd_get_sync_status_label,
            cmd_trigger_manual_sync
        ])
        .run(tauri::generate_context!())
        .expect("error while running PocketLP application");
}
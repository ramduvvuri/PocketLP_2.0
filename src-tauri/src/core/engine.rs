use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const TWELVE_HOURS_MS: i64 = 12 * 60 * 60 * 1000;
pub const FOURTEEN_DAYS_MS: i64 = 14 * 24 * 60 * 60 * 1000;
pub const SCHEMA_SQL: &str = include_str!("../../migrations/001_pocketlp_v1.sql");

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PocketItemDto {
    pub item_id: String,
    pub item_type: String,
    pub title: String,
    pub text_content: Option<String>,
    pub url: Option<String>,
    pub mime_type: Option<String>,
    pub byte_size: Option<i64>,
    pub content_hash: Option<String>,
    pub blob_status: String,
    pub folder_id: Option<String>,
    pub is_trashed: bool,
    pub is_protected: bool,
    pub has_conflict: bool,
    pub aliases: Vec<String>,
    pub updated_at: i64,
    pub last_used_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageBoardItemDto {
    pub board_item_id: String,
    pub title: String,
    pub byte_size: i64,
    pub content_hash: String,
    pub blob_status: String,
    pub captured_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupCandidate {
    pub item_id: String,
    pub title: String,
    pub item_type: String,
    pub byte_size: Option<i64>,
    pub last_used_at: i64,
    pub selected_for_deletion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationRecord {
    pub operation_id: String,
    pub origin_device_id: String,
    pub device_sequence: i64,
    pub lamport_ts: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub op_type: String,
    pub protocol_version: i64,
    pub payload_json: String,
    pub created_at: i64,
}

pub fn init_database(db_path: &Path, default_display_name: &str, clock: &dyn Clock) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA_SQL).map_err(|e| e.to_string())?;

    let now = clock.now_ms();
    conn.execute(
        "INSERT OR IGNORE INTO schema_migrations (version, applied_at) VALUES (1, ?1)",
        params![now],
    )
    .map_err(|e| e.to_string())?;

    let exists: Option<String> = conn
        .query_row("SELECT device_id FROM device_identity WHERE singleton_id = 1", [], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;

    if exists.is_none() {
        let dev_id = Uuid::new_v4().to_string();
        let mut hasher = Sha256::new();
        hasher.update(dev_id.as_bytes());
        let fingerprint = hex::encode(hasher.finalize());
        conn.execute(
            "INSERT INTO device_identity (singleton_id, device_id, display_name, next_sequence, lamport_clock, cert_fingerprint, created_at)
             VALUES (1, ?1, ?2, 1, 1, ?3, ?4)",
            params![dev_id, default_display_name, fingerprint, now],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(conn)
}

pub fn get_device_id(conn: &Connection) -> Result<String, String> {
    conn.query_row("SELECT device_id FROM device_identity WHERE singleton_id = 1", [], |r| r.get(0))
        .map_err(|e| e.to_string())
}

pub fn blob_path(storage_root: &Path, content_hash: &str, is_image_board: bool) -> PathBuf {
    let prefix = &content_hash[0..2.min(content_hash.len())];
    let folder = if is_image_board { "image_board" } else { "files" };
    storage_root.join(folder).join(prefix).join(content_hash)
}

pub fn create_text_or_link_item(
    conn: &mut Connection,
    title: &str,
    content: &str,
    is_link: bool,
    clock: &dyn Clock,
) -> Result<String, String> {
    let now = clock.now_ms();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let item_id = Uuid::new_v4().to_string();
    let (device_id, _, lamport) = next_clock_tx(&tx, None)?;
    let item_type = if is_link { "LINK" } else { "TEXT" };
    let url_val = if is_link { Some(content) } else { None };
    let text_val = if is_link { None } else { Some(content) };

    tx.execute(
        "INSERT INTO items (
            item_id, item_type, title, text_content, url, blob_status,
            created_at, updated_at, last_used_at, lamport_ts, origin_device_id
        ) VALUES (?1, ?2, ?3, ?4, ?5, 'NONE', ?6, ?6, ?6, ?7, ?8)",
        params![item_id, item_type, title, text_val, url_val, now, lamport, device_id],
    )
    .map_err(|e| e.to_string())?;

    append_operation_tx(
        &tx,
        "ITEM",
        &item_id,
        "ITEM_CREATED",
        &serde_json::json!({
            "item_id": item_id,
            "item_type": item_type,
            "title": title,
            "text_content": text_val,
            "url": url_val,
            "created_at": now
        }),
        now,
    )?;
    sync_fts_row(&tx, &item_id)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(item_id)
}

pub fn update_text_item(
    conn: &mut Connection,
    item_id: &str,
    new_title: &str,
    new_text: &str,
    clock: &dyn Clock,
) -> Result<(), String> {
    let now = clock.now_ms();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let (_, _, lamport) = next_clock_tx(&tx, None)?;

    tx.execute(
        "UPDATE items SET title = ?1, text_content = ?2, updated_at = ?3, last_used_at = ?3, lamport_ts = ?4 WHERE item_id = ?5",
        params![new_title, new_text, now, lamport, item_id],
    )
    .map_err(|e| e.to_string())?;

    append_operation_tx(
        &tx,
        "ITEM",
        item_id,
        "ITEM_UPDATED",
        &serde_json::json!({
            "item_id": item_id,
            "title": new_title,
            "text_content": new_text,
            "updated_at": now
        }),
        now,
    )?;
    sync_fts_row(&tx, item_id)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn import_file_atomic(
    conn: &mut Connection,
    storage_root: &Path,
    source_path: &Path,
    original_filename: &str,
    mime_type: &str,
    is_image: bool,
    clock: &dyn Clock,
) -> Result<String, String> {
    let mut reader = BufReader::with_capacity(64 * 1024, File::open(source_path).map_err(|e| e.to_string())?);
    let temp_path = storage_root
        .join("files")
        .join(format!("{}.importing.partial", Uuid::new_v4()));
    if let Some(p) = temp_path.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let mut temp_file = File::create(&temp_path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut total_bytes: i64 = 0;
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        temp_file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        total_bytes += n as i64;
    }
    temp_file.sync_all().map_err(|e| e.to_string())?;
    drop(temp_file);

    let content_hash = hex::encode(hasher.finalize());
    let final_blob = blob_path(storage_root, &content_hash, false);
    if let Some(p) = final_blob.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    if !final_blob.exists() {
        fs::rename(&temp_path, &final_blob).map_err(|e| e.to_string())?;
    } else {
        fs::remove_file(&temp_path).ok();
    }

    let now = clock.now_ms();
    let tx = conn.transaction().map_err(|e| e.to_string())?;

    let existing: Option<(String, String)> = tx
        .query_row(
            "SELECT item_id, title FROM items WHERE content_hash = ?1 LIMIT 1",
            params![content_hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let result_id = if let Some((existing_id, canonical_title)) = existing {
        if canonical_title != original_filename {
            tx.execute(
                "INSERT OR IGNORE INTO item_aliases (item_id, alias_name, added_at) VALUES (?1, ?2, ?3)",
                params![existing_id, original_filename, now],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute(
            "UPDATE items SET is_trashed = 0, blob_status = 'VERIFIED', last_used_at = ?1 WHERE item_id = ?2",
            params![now, existing_id],
        )
        .map_err(|e| e.to_string())?;
        append_operation_tx(
            &tx,
            "ITEM",
            &existing_id,
            "ITEM_UPDATED",
            &serde_json::json!({
                "item_id": existing_id,
                "content_hash": content_hash,
                "added_alias": original_filename
            }),
            now,
        )?;
        sync_fts_row(&tx, &existing_id)?;
        existing_id
    } else {
        let new_id = Uuid::new_v4().to_string();
        let (device_id, _, lamport) = next_clock_tx(&tx, None)?;
        let item_type = if is_image { "IMAGE" } else { "FILE" };
        tx.execute(
            "INSERT INTO items (
                item_id, item_type, title, mime_type, byte_size, content_hash,
                blob_status, created_at, updated_at, last_used_at, lamport_ts, origin_device_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'VERIFIED', ?7, ?7, ?7, ?8, ?9)",
            params![new_id, item_type, original_filename, mime_type, total_bytes, content_hash, now, lamport, device_id],
        )
        .map_err(|e| e.to_string())?;
        append_operation_tx(
            &tx,
            "ITEM",
            &new_id,
            "ITEM_CREATED",
            &serde_json::json!({
                "item_id": new_id,
                "item_type": item_type,
                "title": original_filename,
                "mime_type": mime_type,
                "byte_size": total_bytes,
                "content_hash": content_hash,
                "created_at": now
            }),
            now,
        )?;
        sync_fts_row(&tx, &new_id)?;
        new_id
    };

    tx.commit().map_err(|e| e.to_string())?;
    Ok(result_id)
}

pub fn set_item_trashed(conn: &mut Connection, item_id: &str, trashed: bool, clock: &dyn Clock) -> Result<(), String> {
    let now = clock.now_ms();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let (_, _, lamport) = next_clock_tx(&tx, None)?;
    let flag = if trashed { 1 } else { 0 };
    let trashed_at = if trashed { Some(now) } else { None };
    tx.execute(
        "UPDATE items SET is_trashed = ?1, trashed_at = ?2, updated_at = ?3, lamport_ts = ?4 WHERE item_id = ?5",
        params![flag, trashed_at, now, lamport, item_id],
    )
    .map_err(|e| e.to_string())?;
    let op_type = if trashed { "ITEM_TRASHED" } else { "ITEM_RESTORED" };
    append_operation_tx(
        &tx,
        "ITEM",
        item_id,
        op_type,
        &serde_json::json!({ "item_id": item_id, "is_trashed": trashed, "updated_at": now }),
        now,
    )?;
    sync_fts_row(&tx, item_id)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn permanently_delete_item(
    conn: &mut Connection,
    storage_root: &Path,
    item_id: &str,
    clock: &dyn Clock,
) -> Result<(), String> {
    let now = clock.now_ms();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let content_hash: Option<String> = tx
        .query_row("SELECT content_hash FROM items WHERE item_id = ?1", params![item_id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();

    let (device_id, _, lamport) = next_clock_tx(&tx, None)?;
    let op_id = append_operation_tx(
        &tx,
        "ITEM",
        item_id,
        "ITEM_PERMANENTLY_DELETED",
        &serde_json::json!({ "item_id": item_id, "content_hash": content_hash, "deleted_at": now }),
        now,
    )?;

    tx.execute(
        "INSERT OR REPLACE INTO tombstones (entity_type, entity_id, content_hash, deleted_at, lamport_ts, origin_device_id, operation_id)
         VALUES ('ITEM', ?1, ?2, ?3, ?4, ?5, ?6)",
        params![item_id, content_hash, now, lamport, device_id, op_id],
    )
    .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM items_fts WHERE item_id = ?1", params![item_id])
        .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM items WHERE item_id = ?1", params![item_id])
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;

    if let Some(hash) = content_hash {
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM items WHERE content_hash = ?1", params![hash], |r| r.get(0))
            .unwrap_or(1);
        if remaining == 0 {
            let p = blob_path(storage_root, &hash, false);
            if p.exists() {
                fs::remove_file(p).ok();
            }
        }
    }
    Ok(())
}

pub fn create_image_board_item(
    conn: &mut Connection,
    board_item_id: &str,
    title: &str,
    byte_size: i64,
    content_hash: &str,
    captured_at: i64,
    clock: &dyn Clock,
) -> Result<(), String> {
    let now = clock.now_ms();
    let expires_at = captured_at + TWELVE_HOURS_MS;
    if expires_at <= now {
        return Ok(());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let (device_id, _, lamport) = next_clock_tx(&tx, None)?;
    tx.execute(
        "INSERT OR REPLACE INTO image_board_items (
            board_item_id, title, mime_type, byte_size, content_hash, blob_status,
            captured_at, expires_at, lamport_ts, origin_device_id
        ) VALUES (?1, ?2, 'image/png', ?3, ?4, 'VERIFIED', ?5, ?6, ?7, ?8)",
        params![board_item_id, title, byte_size, content_hash, captured_at, expires_at, lamport, device_id],
    )
    .map_err(|e| e.to_string())?;

    append_operation_tx(
        &tx,
        "IMAGE_BOARD",
        board_item_id,
        "IMAGE_BOARD_ITEM_CREATED",
        &serde_json::json!({
            "board_item_id": board_item_id,
            "title": title,
            "byte_size": byte_size,
            "content_hash": content_hash,
            "captured_at": captured_at,
            "expires_at": expires_at
        }),
        now,
    )?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn expire_stale_image_board_items(
    conn: &mut Connection,
    storage_root: &Path,
    clock: &dyn Clock,
) -> Result<usize, String> {
    let now = clock.now_ms();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut stmt = tx
        .prepare("SELECT board_item_id, content_hash FROM image_board_items WHERE expires_at <= ?1")
        .map_err(|e| e.to_string())?;
    let expired: Vec<(String, String)> = stmt
        .query_map(params![now], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    drop(stmt);

    for (board_id, hash) in &expired {
        let (device_id, _, lamport) = next_clock_tx(&tx, None)?;
        let op_id = append_operation_tx(
            &tx,
            "IMAGE_BOARD",
            board_id,
            "IMAGE_BOARD_ITEM_EXPIRED",
            &serde_json::json!({ "board_item_id": board_id, "expired_at": now }),
            now,
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO tombstones (entity_type, entity_id, content_hash, deleted_at, lamport_ts, origin_device_id, operation_id)
             VALUES ('IMAGE_BOARD', ?1, ?2, ?3, ?4, ?5, ?6)",
            params![board_id, hash, now, lamport, device_id, op_id],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM image_board_items WHERE board_item_id = ?1", params![board_id])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;

    for (_, hash) in &expired {
        let p = blob_path(storage_root, hash, true);
        if p.exists() {
            fs::remove_file(p).ok();
        }
    }
    Ok(expired.len())
}

pub fn list_cleanup_candidates(conn: &Connection, clock: &dyn Clock) -> Result<Vec<CleanupCandidate>, String> {
    let cutoff = clock.now_ms() - FOURTEEN_DAYS_MS;
    let mut stmt = conn
        .prepare(
            "SELECT item_id, title, item_type, byte_size, last_used_at
             FROM items
             WHERE is_trashed = 0
               AND is_protected = 0
               AND has_conflict = 0
               AND blob_status != 'PENDING'
               AND last_used_at <= ?1
             ORDER BY last_used_at ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![cutoff], |r| {
            Ok(CleanupCandidate {
                item_id: r.get(0)?,
                title: r.get(1)?,
                item_type: r.get(2)?,
                byte_size: r.get(3)?,
                last_used_at: r.get(4)?,
                selected_for_deletion: true,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn apply_cleanup_review_decision(
    conn: &mut Connection,
    storage_root: &Path,
    selected_for_delete_ids: &[String],
    deselected_protect_forever_ids: &[String],
    clock: &dyn Clock,
) -> Result<(usize, usize), String> {
    let now = clock.now_ms();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for protect_id in deselected_protect_forever_ids {
        let (_, _, lamport) = next_clock_tx(&tx, None)?;
        tx.execute(
            "UPDATE items SET is_protected = 1, updated_at = ?1, lamport_ts = ?2 WHERE item_id = ?3",
            params![now, lamport, protect_id],
        )
        .map_err(|e| e.to_string())?;
        append_operation_tx(
            &tx,
            "ITEM",
            protect_id,
            "ITEM_PROTECTED",
            &serde_json::json!({ "item_id": protect_id, "is_protected": true }),
            now,
        )?;
    }
    tx.commit().map_err(|e| e.to_string())?;

    for delete_id in selected_for_delete_ids {
        permanently_delete_item(conn, storage_root, delete_id, clock)?;
    }
    Ok((selected_for_delete_ids.len(), deselected_protect_forever_ids.len()))
}

pub fn apply_remote_operation(
    conn: &mut Connection,
    storage_root: &Path,
    op: &OperationRecord,
    clock: &dyn Clock,
) -> Result<bool, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;

    // Idempotency check
    let already_applied: Option<String> = tx
        .query_row(
            "SELECT operation_id FROM operations_log WHERE operation_id = ?1",
            params![op.operation_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if already_applied.is_some() {
        return Ok(false);
    }

    // Advance local Lamport clock above remote Lamport timestamp
    let _ = next_clock_tx(&tx, Some(op.lamport_ts))?;

    tx.execute(
        "INSERT INTO operations_log (
            operation_id, origin_device_id, device_sequence, lamport_ts,
            entity_type, entity_id, op_type, protocol_version, payload_json, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            op.operation_id,
            op.origin_device_id,
            op.device_sequence,
            op.lamport_ts,
            op.entity_type,
            op.entity_id,
            op.op_type,
            op.protocol_version,
            op.payload_json,
            op.created_at
        ],
    )
    .map_err(|e| e.to_string())?;

    // Never resurrect tombstoned entities
    let is_tombstoned: Option<String> = tx
        .query_row(
            "SELECT operation_id FROM tombstones WHERE entity_type = ?1 AND entity_id = ?2",
            params![op.entity_type, op.entity_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if is_tombstoned.is_some() && op.op_type != "ITEM_PERMANENTLY_DELETED" {
        tx.commit().map_err(|e| e.to_string())?;
        return Ok(true);
    }

    let payload: serde_json::Value = serde_json::from_str(&op.payload_json).map_err(|e| e.to_string())?;
    let now = clock.now_ms();

    match op.op_type.as_str() {
        "ITEM_CREATED" => {
            let item_type = payload["item_type"].as_str().unwrap_or("TEXT");
            let title = payload["title"].as_str().unwrap_or("Untitled");
            let text_content = payload["text_content"].as_str();
            let url = payload["url"].as_str();
            let mime_type = payload["mime_type"].as_str();
            let byte_size = payload["byte_size"].as_i64();
            let content_hash = payload["content_hash"].as_str();

            let blob_status = if let Some(h) = content_hash {
                if blob_path(storage_root, h, false).exists() {
                    "VERIFIED"
                } else {
                    "PENDING"
                }
            } else {
                "NONE"
            };

            tx.execute(
                "INSERT OR IGNORE INTO items (
                    item_id, item_type, title, text_content, url, mime_type, byte_size,
                    content_hash, blob_status, created_at, updated_at, last_used_at, lamport_ts, origin_device_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?10, ?11, ?12)",
                params![
                    op.entity_id,
                    item_type,
                    title,
                    text_content,
                    url,
                    mime_type,
                    byte_size,
                    content_hash,
                    blob_status,
                    op.created_at,
                    op.lamport_ts,
                    op.origin_device_id
                ],
            )
            .map_err(|e| e.to_string())?;
            sync_fts_row(&tx, &op.entity_id)?;
        }
        "ITEM_UPDATED" => {
            let existing: Option<(String, Option<String>, i64, String)> = tx
                .query_row(
                    "SELECT title, text_content, lamport_ts, origin_device_id FROM items WHERE item_id = ?1",
                    params![op.entity_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?;

            if let Some((cur_title, cur_text, cur_lamport, cur_dev)) = existing {
                if let Some(alias) = payload["added_alias"].as_str() {
                    tx.execute(
                        "INSERT OR IGNORE INTO item_aliases (item_id, alias_name, added_at) VALUES (?1, ?2, ?3)",
                        params![op.entity_id, alias, now],
                    )
                    .map_err(|e| e.to_string())?;
                }
                let incoming_text = payload["text_content"].as_str();
                let incoming_title = payload["title"].as_str().unwrap_or(&cur_title);

                // Preserve conflict copy if concurrent text edits occurred
                if let (Some(local_t), Some(remote_t)) = (cur_text.as_deref(), incoming_text) {
                    if local_t != remote_t && cur_dev != op.origin_device_id {
                        let conflict_id = format!("{}-conflict-{}", op.entity_id, &op.operation_id[0..8]);
                        tx.execute(
                            "INSERT OR IGNORE INTO items (
                                item_id, item_type, title, text_content, blob_status, has_conflict,
                                conflict_parent_id, created_at, updated_at, last_used_at, lamport_ts, origin_device_id
                            ) VALUES (?1, 'TEXT', ?2, ?3, 'NONE', 1, ?4, ?5, ?5, ?5, ?6, ?7)",
                            params![
                                conflict_id,
                                format!("{} [Conflict Copy]", incoming_title),
                                local_t,
                                op.entity_id,
                                now,
                                cur_lamport,
                                cur_dev
                            ],
                        )
                        .map_err(|e| e.to_string())?;
                        sync_fts_row(&tx, &conflict_id)?;
                    }
                }

                // Total ordering: (lamport_ts, origin_device_id, operation_id)
                if (op.lamport_ts, op.origin_device_id.as_str()) >= (cur_lamport, cur_dev.as_str()) {
                    tx.execute(
                        "UPDATE items SET title = ?1, text_content = COALESCE(?2, text_content), updated_at = ?3, lamport_ts = ?4, origin_device_id = ?5 WHERE item_id = ?6",
                        params![incoming_title, incoming_text, op.created_at, op.lamport_ts, op.origin_device_id, op.entity_id],
                    )
                    .map_err(|e| e.to_string())?;
                }
                sync_fts_row(&tx, &op.entity_id)?;
            }
        }
        "ITEM_TRASHED" | "ITEM_RESTORED" => {
            let trashed = if op.op_type == "ITEM_TRASHED" { 1 } else { 0 };
            tx.execute(
                "UPDATE items SET is_trashed = ?1, updated_at = ?2, lamport_ts = ?3 WHERE item_id = ?4 AND lamport_ts <= ?3",
                params![trashed, op.created_at, op.lamport_ts, op.entity_id],
            )
            .map_err(|e| e.to_string())?;
            sync_fts_row(&tx, &op.entity_id)?;
        }
        "ITEM_PROTECTED" => {
            tx.execute(
                "UPDATE items SET is_protected = 1, lamport_ts = ?1 WHERE item_id = ?2",
                params![op.lamport_ts, op.entity_id],
            )
            .map_err(|e| e.to_string())?;
        }
        "ITEM_PERMANENTLY_DELETED" => {
            let content_hash = payload["content_hash"].as_str();
            tx.execute(
                "INSERT OR REPLACE INTO tombstones (entity_type, entity_id, content_hash, deleted_at, lamport_ts, origin_device_id, operation_id)
                 VALUES ('ITEM', ?1, ?2, ?3, ?4, ?5, ?6)",
                params![op.entity_id, content_hash, op.created_at, op.lamport_ts, op.origin_device_id, op.operation_id],
            )
            .map_err(|e| e.to_string())?;
            tx.execute("DELETE FROM items_fts WHERE item_id = ?1", params![op.entity_id])
                .map_err(|e| e.to_string())?;
            tx.execute("DELETE FROM items WHERE item_id = ?1", params![op.entity_id])
                .map_err(|e| e.to_string())?;
        }
        "IMAGE_BOARD_ITEM_CREATED" => {
            let expires_at = payload["expires_at"].as_i64().unwrap_or(0);
            if expires_at > now {
                let title = payload["title"].as_str().unwrap_or("Screenshot");
                let byte_size = payload["byte_size"].as_i64().unwrap_or(0);
                let content_hash = payload["content_hash"].as_str().unwrap_or("");
                let captured_at = payload["captured_at"].as_i64().unwrap_or(op.created_at);
                tx.execute(
                    "INSERT OR IGNORE INTO image_board_items (
                        board_item_id, title, mime_type, byte_size, content_hash, blob_status,
                        captured_at, expires_at, lamport_ts, origin_device_id
                    ) VALUES (?1, ?2, 'image/png', ?3, ?4, 'PENDING', ?5, ?6, ?7, ?8)",
                    params![op.entity_id, title, byte_size, content_hash, captured_at, expires_at, op.lamport_ts, op.origin_device_id],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        _ => {}
    }

    tx.commit().map_err(|e| e.to_string())?;
    Ok(true)
}

pub fn list_items(conn: &Connection, search: Option<&str>) -> Result<Vec<PocketItemDto>, String> {
    let mut items = Vec::new();
    let sql = if search.is_some() {
        "SELECT i.item_id, i.item_type, i.title, i.text_content, i.url, i.mime_type, i.byte_size,
                i.content_hash, i.blob_status, i.folder_id, i.is_trashed, i.is_protected, i.has_conflict,
                i.updated_at, i.last_used_at
         FROM items i
         JOIN items_fts f ON i.item_id = f.item_id
         WHERE items_fts MATCH ?1
         ORDER BY i.updated_at DESC"
    } else {
        "SELECT item_id, item_type, title, text_content, url, mime_type, byte_size,
                content_hash, blob_status, folder_id, is_trashed, is_protected, has_conflict,
                updated_at, last_used_at
         FROM items
         ORDER BY updated_at DESC"
    };

    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let mapper = |r: &rusqlite::Row| -> rusqlite::Result<PocketItemDto> {
        let id: String = r.get(0)?;
        Ok(PocketItemDto {
            item_id: id,
            item_type: r.get(1)?,
            title: r.get(2)?,
            text_content: r.get(3)?,
            url: r.get(4)?,
            mime_type: r.get(5)?,
            byte_size: r.get(6)?,
            content_hash: r.get(7)?,
            blob_status: r.get(8)?,
            folder_id: r.get(9)?,
            is_trashed: r.get::<_, i64>(10)? != 0,
            is_protected: r.get::<_, i64>(11)? != 0,
            has_conflict: r.get::<_, i64>(12)? != 0,
            aliases: Vec::new(),
            updated_at: r.get(13)?,
            last_used_at: r.get(14)?,
        })
    };

    let rows = if let Some(q) = search {
        stmt.query_map(params![q], mapper).map_err(|e| e.to_string())?
    } else {
        stmt.query_map([], mapper).map_err(|e| e.to_string())?
    };

    for row in rows {
        let mut dto = row.map_err(|e| e.to_string())?;
        let mut alias_stmt = conn
            .prepare("SELECT alias_name FROM item_aliases WHERE item_id = ?1")
            .map_err(|e| e.to_string())?;
        let aliases: Vec<String> = alias_stmt
            .query_map(params![dto.item_id], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        dto.aliases = aliases;
        items.push(dto);
    }
    Ok(items)
}

fn next_clock_tx(tx: &Transaction, remote_lamport: Option<i64>) -> Result<(String, i64, i64), String> {
    let (device_id, seq, local_lamport): (String, i64, i64) = tx
        .query_row(
            "SELECT device_id, next_sequence, lamport_clock FROM device_identity WHERE singleton_id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| e.to_string())?;

    let new_lamport = match remote_lamport {
        Some(rem) => local_lamport.max(rem) + 1,
        None => local_lamport + 1,
    };
    tx.execute(
        "UPDATE device_identity SET next_sequence = ?1, lamport_clock = ?2 WHERE singleton_id = 1",
        params![seq + 1, new_lamport],
    )
    .map_err(|e| e.to_string())?;
    Ok((device_id, seq, new_lamport))
}

fn append_operation_tx(
    tx: &Transaction,
    entity_type: &str,
    entity_id: &str,
    op_type: &str,
    payload: &serde_json::Value,
    now_ms: i64,
) -> Result<String, String> {
    let (device_id, seq, lamport) = next_clock_tx(tx, None)?;
    let op_id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO operations_log (
            operation_id, origin_device_id, device_sequence, lamport_ts,
            entity_type, entity_id, op_type, protocol_version, payload_json, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9)",
        params![
            op_id,
            device_id,
            seq,
            lamport,
            entity_type,
            entity_id,
            op_type,
            payload.to_string(),
            now_ms
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(op_id)
}

fn sync_fts_row(tx: &Transaction, item_id: &str) -> Result<(), String> {
    tx.execute("DELETE FROM items_fts WHERE item_id = ?1", params![item_id])
        .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO items_fts (item_id, title, text_content, url, aliases)
         SELECT i.item_id, i.title, COALESCE(i.text_content, ''), COALESCE(i.url, ''),
                COALESCE((SELECT GROUP_CONCAT(alias_name, ' ') FROM item_aliases a WHERE a.item_id = i.item_id), '')
         FROM items i WHERE i.item_id = ?1 AND i.is_trashed = 0",
        params![item_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
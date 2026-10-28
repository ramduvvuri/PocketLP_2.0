use crate::core::engine::{blob_path, Clock};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const CHUNK_SIZE_BYTES: usize = 4 * 1024 * 1024; // 4 MB chunks

/// Verifies whether a peer is paired and its certificate SHA-256 fingerprint matches the trusted record.
pub fn authenticate_peer(conn: &Connection, peer_device_id: &str, presented_fingerprint: &str) -> Result<bool, String> {
    let expected: Option<String> = conn
        .query_row(
            "SELECT cert_fingerprint_sha256 FROM trusted_peers WHERE peer_device_id = ?1",
            params![peer_device_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    match expected {
        Some(fp) => Ok(fp == presented_fingerprint),
        None => Ok(false),
    }
}

/// Registers or updates a trusted peer after explicit 6-digit verification code approval.
pub fn pair_trusted_peer(
    conn: &Connection,
    peer_device_id: &str,
    display_name: &str,
    cert_fingerprint_sha256: &str,
    endpoint: Option<&str>,
    clock: &dyn Clock,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO trusted_peers (
            peer_device_id, display_name, cert_fingerprint_sha256, last_known_endpoint, paired_at
        ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![peer_device_id, display_name, cert_fingerprint_sha256, endpoint, clock.now_ms()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Resumable chunked file receiver. Appends a chunk at `offset` into `<hash>.partial`
/// and atomically finalizes only when exact byte count and SHA-256 match.
pub fn write_and_verify_transfer_chunk(
    conn: &Connection,
    storage_root: &Path,
    content_hash: &str,
    expected_total_bytes: u64,
    offset: u64,
    chunk_bytes: &[u8],
    is_image_board: bool,
    clock: &dyn Clock,
) -> Result<bool, String> {
    let final_target = blob_path(storage_root, content_hash, is_image_board);
    if final_target.exists() {
        return Ok(true);
    }

    let partial_dir = storage_root.join("partials");
    fs::create_dir_all(&partial_dir).map_err(|e| e.to_string())?;
    let partial_path = partial_dir.join(format!("{}.partial", content_hash));

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .read(true)
        .open(&partial_path)
        .map_err(|e| e.to_string())?;

    file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    file.write_all(chunk_bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;

    let new_completed = offset + chunk_bytes.len() as u64;
    let namespace = if is_image_board { "IMAGE_BOARD" } else { "POCKET" };
    conn.execute(
        "INSERT OR REPLACE INTO file_transfers (
            content_hash, target_namespace, total_bytes, completed_bytes, partial_path, status, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, 'ACTIVE', ?6)",
        params![
            content_hash,
            namespace,
            expected_total_bytes as i64,
            new_completed as i64,
            partial_path.to_string_lossy().to_string(),
            clock.now_ms()
        ],
    )
    .map_err(|e| e.to_string())?;

    if new_completed < expected_total_bytes {
        return Ok(false);
    }

    // Verify full size and SHA-256 streaming without loading all bytes into memory
    drop(file);
    let meta_len = fs::metadata(&partial_path).map_err(|e| e.to_string())?.len();
    if meta_len != expected_total_bytes {
        fs::remove_file(&partial_path).ok();
        return Err("Finalized byte count mismatch".to_string());
    }

    let mut verify_reader = File::open(&partial_path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = verify_reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual_hash = hex::encode(hasher.finalize());
    if actual_hash != content_hash {
        fs::remove_file(&partial_path).ok();
        return Err("SHA-256 hash verification failed".to_string());
    }

    if let Some(parent) = final_target.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::rename(&partial_path, &final_target).map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE items SET blob_status = 'VERIFIED' WHERE content_hash = ?1",
        params![content_hash],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE file_transfers SET status = 'COMPLETED', updated_at = ?1 WHERE content_hash = ?2",
        params![clock.now_ms(), content_hash],
    )
    .map_err(|e| e.to_string())?;

    Ok(true)
}
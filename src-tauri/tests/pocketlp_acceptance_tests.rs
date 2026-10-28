use pocketlp_lib::core::engine::{
    apply_cleanup_review_decision, apply_remote_operation, create_image_board_item,
    create_text_or_link_item, expire_stale_image_board_items, get_device_id, import_file_atomic,
    init_database, list_cleanup_candidates, list_items, permanently_delete_item, set_item_trashed,
    update_text_item, Clock, OperationRecord, FOURTEEN_DAYS_MS, TWELVE_HOURS_MS,
};
use pocketlp_lib::core::network_sync::{
    authenticate_peer, pair_trusted_peer, write_and_verify_transfer_chunk,
};
use std::fs;
use std::sync::atomic::{AtomicI64, Ordering};
use tempfile::TempDir;

struct FakeClock {
    now: AtomicI64,
}
impl FakeClock {
    fn new(start: i64) -> Self {
        Self {
            now: AtomicI64::new(start),
        }
    }
    fn advance(&self, delta_ms: i64) {
        self.now.fetch_add(delta_ms, Ordering::SeqCst);
    }
}
impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }
}

#[test]
fn test_device_identity_persists_across_restart() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("lp.db");
    let clock = FakeClock::new(1_700_000_000_000);

    let conn1 = init_database(&db_path, "Lenovo LOQ", &clock).unwrap();
    let id1 = get_device_id(&conn1).unwrap();
    drop(conn1);

    let conn2 = init_database(&db_path, "Lenovo LOQ", &clock).unwrap();
    let id2 = get_device_id(&conn2).unwrap();
    assert_eq!(id1, id2);
}

#[test]
fn test_sha256_file_deduplication_and_aliases() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("lp.db");
    let clock = FakeClock::new(1_700_000_000_000);
    let mut conn = init_database(&db_path, "Lenovo LOQ", &clock).unwrap();

    let f1 = dir.path().join("notes_v1.pdf");
    let f2 = dir.path().join("notes_copy.pdf");
    fs::write(&f1, b"identical-pdf-binary-payload").unwrap();
    fs::write(&f2, b"identical-pdf-binary-payload").unwrap();

    let id1 = import_file_atomic(&mut conn, dir.path(), &f1, "notes_v1.pdf", "application/pdf", false, &clock).unwrap();
    let id2 = import_file_atomic(&mut conn, dir.path(), &f2, "notes_copy.pdf", "application/pdf", false, &clock).unwrap();

    assert_eq!(id1, id2, "Identical SHA-256 bytes must map to one logical item");
    let all = list_items(&conn, None).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].aliases, vec!["notes_copy.pdf".to_string()]);
}

#[test]
fn test_idempotent_remote_ops_and_conflict_copy_preservation() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("lp.db");
    let clock = FakeClock::new(1_700_000_000_000);
    let mut conn = init_database(&db_path, "Lenovo LOQ", &clock).unwrap();

    let item_id = create_text_or_link_item(&mut conn, "Shared Note", "Laptop offline edit", false, &clock).unwrap();
    update_text_item(&mut conn, &item_id, "Shared Note", "Laptop version v2", &clock).unwrap();

    let remote_op = OperationRecord {
        operation_id: "op-remote-999".to_string(),
        origin_device_id: "iqoo-z9-device-uuid".to_string(),
        device_sequence: 1,
        lamport_ts: 50,
        entity_type: "ITEM".to_string(),
        entity_id: item_id.clone(),
        op_type: "ITEM_UPDATED".to_string(),
        protocol_version: 1,
        payload_json: serde_json::json!({
            "item_id": item_id,
            "title": "Shared Note",
            "text_content": "Android concurrent offline edit"
        })
        .to_string(),
        created_at: clock.now_ms(),
    };

    let applied_first = apply_remote_operation(&mut conn, dir.path(), &remote_op, &clock).unwrap();
    let applied_second = apply_remote_operation(&mut conn, dir.path(), &remote_op, &clock).unwrap();
    assert!(applied_first);
    assert!(!applied_second, "Duplicate operation must be a harmless no-op");

    let items = list_items(&conn, None).unwrap();
    assert_eq!(items.len(), 2, "Both the winning note and the conflict copy must be preserved");
}

#[test]
fn test_trash_restore_and_tombstone_prevents_resurrection() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("lp.db");
    let clock = FakeClock::new(1_700_000_000_000);
    let mut conn = init_database(&db_path, "Lenovo LOQ", &clock).unwrap();

    let item_id = create_text_or_link_item(&mut conn, "Temp Secret", "Delete me", false, &clock).unwrap();
    set_item_trashed(&mut conn, &item_id, true, &clock).unwrap();
    set_item_trashed(&mut conn, &item_id, false, &clock).unwrap();
    permanently_delete_item(&mut conn, dir.path(), &item_id, &clock).unwrap();

    // Late create operation from an offline replica must NOT resurrect a tombstoned item
    let late_create = OperationRecord {
        operation_id: "op-late-create".to_string(),
        origin_device_id: "iqoo-z9-uuid".to_string(),
        device_sequence: 2,
        lamport_ts: 999,
        entity_type: "ITEM".to_string(),
        entity_id: item_id.clone(),
        op_type: "ITEM_CREATED".to_string(),
        protocol_version: 1,
        payload_json: serde_json::json!({
            "item_id": item_id,
            "item_type": "TEXT",
            "title": "Resurrected?"
        })
        .to_string(),
        created_at: clock.now_ms(),
    };
    apply_remote_operation(&mut conn, dir.path(), &late_create, &clock).unwrap();
    assert!(list_items(&conn, None).unwrap().is_empty());
}

#[test]
fn test_image_board_12_hour_expiry_from_capture_time() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("lp.db");
    let clock = FakeClock::new(1_700_000_000_000);
    let mut conn = init_database(&db_path, "Lenovo LOQ", &clock).unwrap();

    let capture_t = clock.now_ms();
    create_image_board_item(&mut conn, "shot-1", "Screen_10am.png", 1024, "abcd1234", capture_t, &clock).unwrap();

    clock.advance(TWELVE_HOURS_MS + 1000);
    let expired_count = expire_stale_image_board_items(&mut conn, dir.path(), &clock).unwrap();
    assert_eq!(expired_count, 1);
}

#[test]
fn test_14_day_cleanup_selected_deletes_and_deselected_protects_forever() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("lp.db");
    let clock = FakeClock::new(1_700_000_000_000);
    let mut conn = init_database(&db_path, "Lenovo LOQ", &clock).unwrap();

    let delete_me = create_text_or_link_item(&mut conn, "Old Stale Item", "Delete", false, &clock).unwrap();
    let keep_me = create_text_or_link_item(&mut conn, "Important Reference", "Keep", false, &clock).unwrap();

    clock.advance(FOURTEEN_DAYS_MS + 5000);
    let candidates = list_cleanup_candidates(&conn, &clock).unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().all(|c| c.selected_for_deletion));

    let (deleted, protected) = apply_cleanup_review_decision(
        &mut conn,
        dir.path(),
        &[delete_me.clone()],
        &[keep_me.clone()],
        &clock,
    )
    .unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(protected, 1);

    // Advance another 30 days: protected item must never reappear as a cleanup candidate
    clock.advance(FOURTEEN_DAYS_MS * 2);
    let next_candidates = list_cleanup_candidates(&conn, &clock).unwrap();
    assert!(next_candidates.is_empty());
}

#[test]
fn test_chunked_resumable_transfer_and_peer_authentication() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("lp.db");
    let clock = FakeClock::new(1_700_000_000_000);
    let conn = init_database(&db_path, "Lenovo LOQ", &clock).unwrap();

    assert!(!authenticate_peer(&conn, "unknown-peer", "fp-123").unwrap());
    pair_trusted_peer(&conn, "iqoo-peer", "iQOO Z9 5G", "fp-verified-sha256", Some("192.168.1.50:7700"), &clock).unwrap();
    assert!(authenticate_peer(&conn, "iqoo-peer", "fp-verified-sha256").unwrap());

    let data = b"chunk-1-payload-and-chunk-2-payload";
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    hasher.update(data);
    let hash = hex::encode(hasher.finalize());

    let done1 = write_and_verify_transfer_chunk(&conn, dir.path(), &hash, data.len() as u64, 0, &data[0..15], false, &clock).unwrap();
    assert!(!done1, "Partial file must not be marked complete early");

    let done2 = write_and_verify_transfer_chunk(&conn, dir.path(), &hash, data.len() as u64, 15, &data[15..], false, &clock).unwrap();
    assert!(done2, "Final chunk must pass SHA-256 verification and finalize atomically");
}
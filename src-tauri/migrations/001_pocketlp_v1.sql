PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS device_identity (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    device_id TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    next_sequence INTEGER NOT NULL DEFAULT 1,
    lamport_clock INTEGER NOT NULL DEFAULT 0,
    cert_fingerprint TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS folders (
    folder_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    lamport_ts INTEGER NOT NULL,
    origin_device_id TEXT NOT NULL,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS items (
    item_id TEXT PRIMARY KEY,
    item_type TEXT NOT NULL CHECK (item_type IN ('TEXT', 'LINK', 'NOTE', 'FILE', 'IMAGE')),
    title TEXT NOT NULL,
    text_content TEXT,
    url TEXT,
    mime_type TEXT,
    byte_size INTEGER,
    content_hash TEXT,
    blob_status TEXT NOT NULL DEFAULT 'NONE' CHECK (blob_status IN ('NONE', 'PENDING', 'VERIFIED')),
    folder_id TEXT REFERENCES folders(folder_id) ON DELETE SET NULL,
    is_trashed INTEGER NOT NULL DEFAULT 0,
    trashed_at INTEGER,
    is_protected INTEGER NOT NULL DEFAULT 0,
    has_conflict INTEGER NOT NULL DEFAULT 0,
    conflict_parent_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    lamport_ts INTEGER NOT NULL,
    origin_device_id TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS item_aliases (
    item_id TEXT NOT NULL REFERENCES items(item_id) ON DELETE CASCADE,
    alias_name TEXT NOT NULL,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (item_id, alias_name)
);

CREATE TABLE IF NOT EXISTS tombstones (
    entity_type TEXT NOT NULL CHECK (entity_type IN ('ITEM', 'FOLDER', 'IMAGE_BOARD')),
    entity_id TEXT NOT NULL,
    content_hash TEXT,
    deleted_at INTEGER NOT NULL,
    lamport_ts INTEGER NOT NULL,
    origin_device_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    PRIMARY KEY (entity_type, entity_id)
);

CREATE TABLE IF NOT EXISTS image_board_items (
    board_item_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    byte_size INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    blob_status TEXT NOT NULL DEFAULT 'PENDING' CHECK (blob_status IN ('PENDING', 'VERIFIED')),
    captured_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    lamport_ts INTEGER NOT NULL,
    origin_device_id TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS operations_log (
    operation_id TEXT PRIMARY KEY,
    origin_device_id TEXT NOT NULL,
    device_sequence INTEGER NOT NULL,
    lamport_ts INTEGER NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    op_type TEXT NOT NULL,
    protocol_version INTEGER NOT NULL DEFAULT 1,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (origin_device_id, device_sequence)
);

CREATE TABLE IF NOT EXISTS trusted_peers (
    peer_device_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    cert_fingerprint_sha256 TEXT NOT NULL,
    last_known_endpoint TEXT,
    acked_local_sequence INTEGER NOT NULL DEFAULT 0,
    highest_remote_sequence INTEGER NOT NULL DEFAULT 0,
    paired_at INTEGER NOT NULL,
    last_sync_at INTEGER
);

CREATE TABLE IF NOT EXISTS file_transfers (
    content_hash TEXT PRIMARY KEY,
    target_namespace TEXT NOT NULL CHECK (target_namespace IN ('POCKET', 'IMAGE_BOARD')),
    total_bytes INTEGER NOT NULL,
    chunk_size INTEGER NOT NULL DEFAULT 4194304,
    completed_bytes INTEGER NOT NULL DEFAULT 0,
    partial_path TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('PENDING', 'ACTIVE', 'PAUSED', 'VERIFYING', 'COMPLETED', 'FAILED')),
    retry_count INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    updated_at INTEGER NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS items_fts USING fts5(
    item_id UNINDEXED,
    title,
    text_content,
    url,
    aliases
);
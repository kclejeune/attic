-- Attic Worker Database Schema
-- Compatible with SQLite (D1/Turso)

-- Cache table
CREATE TABLE IF NOT EXISTS cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    keypair TEXT NOT NULL,
    is_public INTEGER NOT NULL DEFAULT 0,
    store_dir TEXT NOT NULL DEFAULT '/nix/store',
    priority INTEGER NOT NULL DEFAULT 40,
    upstream_cache_key_names TEXT NOT NULL DEFAULT '[]',
    compression TEXT NOT NULL DEFAULT 'br', -- none, zstd, br, gzip
    created_at TEXT NOT NULL,
    deleted_at TEXT,
    retention_period INTEGER
);

CREATE INDEX IF NOT EXISTS idx_cache_name ON cache(name);
CREATE INDEX IF NOT EXISTS idx_cache_deleted ON cache(deleted_at);

-- NAR table (content-addressed)
CREATE TABLE IF NOT EXISTS nar (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    state TEXT NOT NULL DEFAULT 'P', -- V=Valid, P=PendingUpload, C=ConfirmedDeduplicated, D=Deleted
    nar_hash TEXT NOT NULL,
    nar_size INTEGER NOT NULL,
    compression TEXT NOT NULL DEFAULT 'none',
    num_chunks INTEGER NOT NULL DEFAULT 1,
    completeness_hint INTEGER NOT NULL DEFAULT 0,
    holders_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_nar_hash ON nar(nar_hash);
CREATE INDEX IF NOT EXISTS idx_nar_state ON nar(state);

-- Object table (cache-specific view of a NAR)
CREATE TABLE IF NOT EXISTS object (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    cache_id INTEGER NOT NULL REFERENCES cache(id),
    nar_id INTEGER NOT NULL REFERENCES nar(id),
    store_path_hash TEXT NOT NULL,
    store_path TEXT NOT NULL,
    refs TEXT NOT NULL DEFAULT '[]',
    system TEXT,
    deriver TEXT,
    sigs TEXT NOT NULL DEFAULT '[]',
    ca TEXT,
    created_at TEXT NOT NULL,
    last_accessed_at TEXT,
    created_by TEXT,
    UNIQUE(cache_id, store_path_hash)
);

CREATE INDEX IF NOT EXISTS idx_object_cache_hash ON object(cache_id, store_path_hash);
CREATE INDEX IF NOT EXISTS idx_object_nar ON object(nar_id);

-- Chunk table (deduplicated storage units)
CREATE TABLE IF NOT EXISTS chunk (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    state TEXT NOT NULL DEFAULT 'P', -- V=Valid, P=PendingUpload, C=ConfirmedDeduplicated, D=Deleted
    chunk_hash TEXT NOT NULL,
    chunk_size INTEGER NOT NULL,
    file_hash TEXT,
    file_size INTEGER,
    compression TEXT NOT NULL DEFAULT 'none',
    remote_file TEXT NOT NULL, -- JSON-encoded RemoteFile
    remote_file_id TEXT NOT NULL,
    holders_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_chunk_hash ON chunk(chunk_hash, compression);
CREATE INDEX IF NOT EXISTS idx_chunk_state ON chunk(state);

-- ChunkRef table (NAR to chunk mapping)
CREATE TABLE IF NOT EXISTS chunkref (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    nar_id INTEGER NOT NULL REFERENCES nar(id),
    seq INTEGER NOT NULL,
    chunk_id INTEGER REFERENCES chunk(id),
    chunk_hash TEXT NOT NULL,
    compression TEXT NOT NULL DEFAULT 'none'
);

CREATE INDEX IF NOT EXISTS idx_chunkref_nar ON chunkref(nar_id, seq);
CREATE INDEX IF NOT EXISTS idx_chunkref_chunk ON chunkref(chunk_id);

-- Migrations tracking table
CREATE TABLE IF NOT EXISTS _migrations (
    id TEXT PRIMARY KEY,
    applied_at TEXT NOT NULL
);

//! Database models for the Attic Worker.
//!
//! These are plain Rust structs that match the database schema,
//! without any ORM dependencies.

use serde::{Deserialize, Serialize};

/// Outcome of a cache rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameOutcome {
    /// The cache was renamed.
    Renamed,
    /// The source cache does not exist.
    NotFound,
    /// The target name is already taken (by a live or soft-deleted cache).
    Conflict,
}

/// Cache model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cache {
    pub id: Option<i64>,
    pub name: String,
    pub keypair: String,
    pub is_public: bool,
    pub store_dir: String,
    pub priority: i32,
    pub upstream_cache_key_names: Vec<String>,
    /// Compression type: "none", "zstd", or "br" (brotli)
    pub compression: String,
    pub created_at: String,
    pub deleted_at: Option<String>,
    pub retention_period: Option<i32>,
}

/// NAR state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NarState {
    /// Valid and complete.
    #[serde(rename = "V")]
    Valid,
    /// Pending upload.
    #[serde(rename = "P")]
    PendingUpload,
    /// Confirmed deduplicated.
    #[serde(rename = "C")]
    ConfirmedDeduplicated,
    /// Deleted.
    #[serde(rename = "D")]
    Deleted,
}

impl NarState {
    pub fn as_str(&self) -> &'static str {
        match self {
            NarState::Valid => "V",
            NarState::PendingUpload => "P",
            NarState::ConfirmedDeduplicated => "C",
            NarState::Deleted => "D",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "V" => Some(NarState::Valid),
            "P" => Some(NarState::PendingUpload),
            "C" => Some(NarState::ConfirmedDeduplicated),
            "D" => Some(NarState::Deleted),
            _ => None,
        }
    }
}

/// NAR model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nar {
    pub id: Option<i64>,
    pub state: NarState,
    pub nar_hash: String,
    pub nar_size: i64,
    pub compression: String,
    pub num_chunks: i32,
    pub completeness_hint: bool,
    pub holders_count: i32,
    pub created_at: String,
}

/// Chunk state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkState {
    /// Valid and complete.
    #[serde(rename = "V")]
    Valid,
    /// Pending upload.
    #[serde(rename = "P")]
    PendingUpload,
    /// Confirmed deduplicated.
    #[serde(rename = "C")]
    ConfirmedDeduplicated,
    /// Deleted.
    #[serde(rename = "D")]
    Deleted,
}

impl ChunkState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChunkState::Valid => "V",
            ChunkState::PendingUpload => "P",
            ChunkState::ConfirmedDeduplicated => "C",
            ChunkState::Deleted => "D",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "V" => Some(ChunkState::Valid),
            "P" => Some(ChunkState::PendingUpload),
            "C" => Some(ChunkState::ConfirmedDeduplicated),
            "D" => Some(ChunkState::Deleted),
            _ => None,
        }
    }
}

/// Chunk model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: Option<i64>,
    pub state: ChunkState,
    pub chunk_hash: String,
    pub chunk_size: i64,
    pub file_hash: Option<String>,
    pub file_size: Option<i64>,
    pub compression: String,
    pub remote_file: String, // JSON-encoded RemoteFile
    pub remote_file_id: String,
    pub holders_count: i32,
    pub created_at: String,
}

/// Object model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Object {
    pub id: Option<i64>,
    pub cache_id: i64,
    pub nar_id: i64,
    pub store_path_hash: String,
    pub store_path: String,
    pub references: Vec<String>,
    pub system: Option<String>,
    pub deriver: Option<String>,
    pub sigs: Vec<String>,
    pub ca: Option<String>,
    pub created_at: String,
    pub last_accessed_at: Option<String>,
    pub created_by: Option<String>,
}

/// Chunk reference model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRef {
    pub id: Option<i64>,
    pub nar_id: i64,
    pub seq: i32,
    pub chunk_id: Option<i64>,
    pub chunk_hash: String,
    pub compression: String,
}

/// Object with associated NAR data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectWithNar {
    pub object: Object,
    pub nar: Nar,
}

/// An in-progress OAuth device-authorization grant (headless CLI login).
#[derive(Debug, Clone)]
pub struct DeviceAuth {
    pub device_code: String,
    pub user_code: String,
    /// "pending" | "approved" | "denied"
    pub status: String,
    /// Minted token, populated once approved.
    pub token: Option<String>,
    pub expires_at: i64,
}

/// A chunk no longer referenced by any NAR, to be reclaimed by GC.
#[derive(Debug, Clone)]
pub struct OrphanChunk {
    pub id: i64,
    /// JSON-encoded RemoteFile locating the bytes in R2.
    pub remote_file: String,
}

/// Server-side state for an in-progress chunked upload.
///
/// The client holds only the opaque `token`; all trusted fields (target cache,
/// R2 multipart identifiers, part accounting) live here so a client cannot forge
/// them by editing the token. Rows are deleted on completion and reaped by GC if
/// abandoned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingUpload {
    /// Opaque random token identifying this upload.
    pub token: String,
    /// Target cache id.
    pub cache_id: i64,
    /// Target cache name (used to re-check push permission on each request).
    pub cache_name: String,
    /// R2 multipart upload id.
    pub r2_upload_id: String,
    /// R2 multipart upload key.
    pub r2_key: String,
    /// Final storage key for the object.
    pub storage_key: String,
    /// JSON-encoded ChunkedNarInfo for final object creation.
    pub nar_info: String,
    /// Expected uncompressed NAR size.
    pub expected_nar_size: i64,
    /// Compression codec of the stored bytes.
    pub compression: String,
    /// Number of parts uploaded so far.
    pub parts_uploaded: i32,
    /// Total compressed bytes received.
    pub bytes_received: i64,
    /// JSON-encoded Vec<UploadedPartInfo> for multipart completion.
    pub uploaded_parts: String,
    /// RFC3339 creation timestamp.
    pub created_at: String,
}

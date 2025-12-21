//! Database models for the Attic Worker.
//!
//! These are plain Rust structs that match the database schema,
//! without any ORM dependencies.

use serde::{Deserialize, Serialize};

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

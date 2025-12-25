use serde::{Deserialize, Serialize};
use serde_with::{DefaultOnError, serde_as};

use crate::cache::CacheName;
use crate::hash::Hash;
use crate::nix_store::StorePathHash;

/// Header containing the upload info.
pub const ATTIC_NAR_INFO: &str = "X-Attic-Nar-Info";

/// Header containing the size of the upload info at the beginning of the body.
pub const ATTIC_NAR_INFO_PREAMBLE_SIZE: &str = "X-Attic-Nar-Info-Preamble-Size";

/// NAR information associated with a upload.
///
/// There are two ways for the client to supply the NAR information:
///
/// 1. At the beginning of the PUT body. The `X-Attic-Nar-Info-Preamble-Size`
///    header must be set to the size of the JSON.
/// 2. Through the `X-Attic-Nar-Info` header.
///
/// The client is advised to use the first method if the serialized
/// JSON is large (>4K).
///
/// Regardless of client compression, the server will always decompress
/// the NAR to validate the NAR hash before applying the server-configured
/// compression again.
#[derive(Debug, Serialize, Deserialize)]
pub struct UploadPathNarInfo {
    /// The name of the binary cache to upload to.
    pub cache: CacheName,

    /// The hash portion of the store path.
    pub store_path_hash: StorePathHash,

    /// The full store path being cached, including the store directory.
    pub store_path: String,

    /// Other store paths this object directly refereces.
    pub references: Vec<String>,

    /// The system this derivation is built for.
    pub system: Option<String>,

    /// The derivation that produced this object.
    pub deriver: Option<String>,

    /// The signatures of this object.
    pub sigs: Vec<String>,

    /// The CA field of this object.
    pub ca: Option<String>,

    /// The hash of the NAR.
    ///
    /// It must begin with `sha256:` with the SHA-256 hash in the
    /// hexadecimal format (64 hex characters).
    ///
    /// This is informational and the server always validates the supplied
    /// hash.
    pub nar_hash: Hash,

    /// The size of the NAR.
    pub nar_size: usize,
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
pub struct UploadPathResult {
    #[serde_as(deserialize_as = "DefaultOnError")]
    pub kind: UploadPathResultKind,

    /// The compressed size of the NAR, in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_size: Option<usize>,

    /// The fraction of data that was deduplicated, from 0 to 1.
    pub frac_deduplicated: Option<f64>,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UploadPathResultKind {
    /// The path was uploaded.
    ///
    /// This is purely informational and servers may return
    /// this variant even when the NAR is deduplicated.
    #[default]
    Uploaded,

    /// The path was globally deduplicated.
    ///
    /// The exact semantics of what counts as deduplicated
    /// is opaque to the client.
    Deduplicated,
}

// =============================================================================
// Chunked Upload Types
// =============================================================================
// For files larger than Cloudflare's 100MB request limit, we use a chunked
// upload protocol:
//
// 1. POST /_api/v1/upload-path/start - Start chunked upload, get upload token
// 2. PUT /_api/v1/upload-path/chunk - Upload chunks (< 50MB each)
// 3. POST /_api/v1/upload-path/complete - Complete the upload

/// Maximum recommended chunk size (50MB to stay under worker memory limits).
/// Cloudflare Workers have a 128MB memory limit, and we need headroom for
/// request processing, so we use 50MB chunks to balance throughput and safety.
pub const CHUNKED_UPLOAD_CHUNK_SIZE: usize = 50 * 1024 * 1024;

/// Threshold for using chunked uploads (100MB NAR size).
/// Files larger than this will be uploaded in chunks.
pub const CHUNKED_UPLOAD_THRESHOLD: usize = 100 * 1024 * 1024;

/// Request body for starting a chunked upload.
#[derive(Debug, Serialize, Deserialize)]
pub struct StartChunkedUploadRequest {
    /// NAR info for the upload.
    pub nar_info: ChunkedNarInfo,
    /// Expected total NAR size (uncompressed).
    pub nar_size: u64,
}

/// NAR info for chunked uploads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkedNarInfo {
    pub cache: CacheName,
    pub store_path_hash: StorePathHash,
    pub store_path: String,
    pub references: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deriver: Option<String>,
    pub sigs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca: Option<String>,
    pub nar_hash: Hash,
}

/// Response for starting a chunked upload.
#[derive(Debug, Serialize, Deserialize)]
pub struct StartChunkedUploadResponse {
    /// Opaque upload token.
    pub upload_token: String,
    /// Recommended chunk size.
    pub chunk_size: u64,
}

/// Result of starting a chunked upload - either proceed with upload or already deduplicated.
#[derive(Debug)]
pub enum StartChunkedUploadResult {
    /// Proceed with chunked upload using this token.
    Proceed(StartChunkedUploadResponse),
    /// NAR was deduplicated, upload complete.
    Deduplicated(UploadPathResult),
}

/// Response for uploading a chunk.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChunkUploadResponse {
    /// Updated upload token (must be used for subsequent chunks).
    pub upload_token: String,
    /// Number of parts uploaded so far.
    pub parts_uploaded: u16,
    /// Total bytes received (compressed).
    pub bytes_received: u64,
}

/// Request body for completing a chunked upload.
#[derive(Debug, Serialize, Deserialize)]
pub struct CompleteChunkedUploadRequest {
    /// Upload token from the last chunk upload.
    pub upload_token: String,
}

impl From<&UploadPathNarInfo> for ChunkedNarInfo {
    fn from(info: &UploadPathNarInfo) -> Self {
        Self {
            cache: info.cache.clone(),
            store_path_hash: info.store_path_hash.clone(),
            store_path: info.store_path.clone(),
            references: info.references.clone(),
            system: info.system.clone(),
            deriver: info.deriver.clone(),
            sigs: info.sigs.clone(),
            ca: info.ca.clone(),
            nar_hash: info.nar_hash.clone(),
        }
    }
}

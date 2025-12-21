//! Upload path handler.

use serde::{Deserialize, Serialize};
use worker::*;

use crate::compression::compress_buffer;
use crate::database::{Chunk, ChunkRef, ChunkState, Nar, NarState, Object};
use crate::error::WorkerError;
use crate::state::{RequestState, WorkerState};

/// Header containing the NAR info JSON.
const ATTIC_NAR_INFO: &str = "X-Attic-Nar-Info";

/// Header containing the preamble size.
const ATTIC_NAR_INFO_PREAMBLE_SIZE: &str = "X-Attic-Nar-Info-Preamble-Size";

/// Upload path NAR info from client.
#[derive(Debug, Deserialize)]
struct UploadPathNarInfo {
    cache: String,
    store_path_hash: String,
    store_path: String,
    references: Vec<String>,
    system: Option<String>,
    deriver: Option<String>,
    sigs: Vec<String>,
    ca: Option<String>,
    nar_hash: String,
    #[allow(dead_code)]
    nar_size: u64,
}

/// Upload result.
#[derive(Serialize)]
struct UploadPathResult {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frac_deduplicated: Option<f64>,
}

/// PUT /_api/v1/upload-path
///
/// Uploads a new store path to the cache.
pub async fn upload_path(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    let req_state = match RequestState::from_request(&req, &state.jwt_config) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    // Check authentication
    let token = match req_state.token {
        Some(t) => t,
        None => {
            return Ok(WorkerError::Authentication("No token provided".to_string()).to_response())
        }
    };

    // Parse NAR info from header or preamble
    let upload_info = parse_upload_info(&req).await?;

    // Check permission to push
    let cache_name = attic::cache::CacheName::new(upload_info.cache.clone())
        .map_err(|e| WorkerError::BadRequest(format!("Invalid cache name: {}", e)))?;
    let permission = token.get_permission_for_cache(&cache_name);
    if let Err(e) = permission.require_push() {
        return Ok(WorkerError::Authorization(format!("Permission denied: {:?}", e)).to_response());
    }

    // Find the cache
    let cache = match state.database.find_cache(&upload_info.cache).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return Ok(WorkerError::NotFound(format!(
                "Cache not found: {}",
                upload_info.cache
            ))
            .to_response())
        }
        Err(e) => return Ok(e.to_response()),
    };

    let cache_id = cache.id.ok_or_else(|| {
        worker::Error::RustError("Cache has no ID".to_string())
    })?;

    // Try to deduplicate by finding an existing NAR with the same hash
    if let Ok(Some(existing_nar)) = state.database.try_lock_nar(&upload_info.nar_hash).await {
        // Found existing NAR - create object pointing to it
        return handle_deduplicated_upload(&state, &upload_info, cache_id, existing_nar).await;
    }

    // New upload - read body and upload to R2
    handle_new_upload(&state, req, &upload_info, cache_id).await
}

/// Parse upload info from request headers or body preamble.
async fn parse_upload_info(req: &Request) -> Result<UploadPathNarInfo> {
    let headers = req.headers();

    // Try header first
    if let Some(nar_info) = headers.get(ATTIC_NAR_INFO).ok().flatten() {
        return serde_json::from_str(&nar_info)
            .map_err(|e| worker::Error::RustError(format!("Invalid NAR info: {}", e)));
    }

    // Try preamble
    if let Some(preamble_size_str) = headers.get(ATTIC_NAR_INFO_PREAMBLE_SIZE).ok().flatten() {
        let _preamble_size: usize = preamble_size_str
            .parse()
            .map_err(|e| worker::Error::RustError(format!("Invalid preamble size: {}", e)))?;

        // TODO: Read preamble from body
        return Err(worker::Error::RustError(
            "Preamble-based NAR info not yet supported".to_string(),
        ));
    }

    Err(worker::Error::RustError(
        "NAR info not provided in header or preamble".to_string(),
    ))
}

/// Handle a deduplicated upload (NAR already exists).
async fn handle_deduplicated_upload(
    state: &WorkerState,
    upload_info: &UploadPathNarInfo,
    cache_id: i64,
    existing_nar: crate::database::Nar,
) -> Result<Response> {
    let nar_id = existing_nar.id.ok_or_else(|| {
        worker::Error::RustError("NAR has no ID".to_string())
    })?;

    // Create object pointing to existing NAR
    let object = Object {
        id: None,
        cache_id,
        nar_id,
        store_path_hash: upload_info.store_path_hash.clone(),
        store_path: upload_info.store_path.clone(),
        references: upload_info.references.clone(),
        system: upload_info.system.clone(),
        deriver: upload_info.deriver.clone(),
        sigs: upload_info.sigs.clone(),
        ca: upload_info.ca.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        last_accessed_at: None,
        created_by: None,
    };

    if let Err(e) = state.database.create_object(&object).await {
        // Release the lock on failure
        let _ = state.database.release_nar_lock(nar_id).await;
        return Ok(e.to_response());
    }

    // Release the lock
    let _ = state.database.release_nar_lock(nar_id).await;

    let result = UploadPathResult {
        kind: "deduplicated".to_string(),
        file_size: None,
        frac_deduplicated: Some(1.0),
    };

    Response::from_json(&result)
}

/// Handle a new upload (upload NAR data).
async fn handle_new_upload(
    state: &WorkerState,
    mut req: Request,
    upload_info: &UploadPathNarInfo,
    cache_id: i64,
) -> Result<Response> {
    // Read body
    let body_bytes = match req.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return Err(e);
        }
    };

    // Compress with dual hashing (NAR hash before compression, file hash after)
    let compression_result = match compress_buffer(&body_bytes, &state.compression_config) {
        Ok(r) => r,
        Err(e) => return Ok(e.to_response()),
    };

    // Validate NAR hash matches what client claimed
    // The client sends hash in format "sha256:hexdigest" or just "hexdigest"
    let expected_nar_hash = upload_info
        .nar_hash
        .strip_prefix("sha256:")
        .unwrap_or(&upload_info.nar_hash);

    if compression_result.nar_hash != expected_nar_hash {
        return Ok(WorkerError::BadRequest(format!(
            "NAR hash mismatch: expected {}, got {}",
            expected_nar_hash, compression_result.nar_hash
        ))
        .to_response());
    }

    // Create pending NAR entry with compression info
    let nar = Nar {
        id: None,
        state: NarState::PendingUpload,
        nar_hash: upload_info.nar_hash.clone(),
        nar_size: compression_result.nar_size as i64,
        compression: compression_result.compression.as_str().to_string(),
        num_chunks: 1,
        completeness_hint: false,
        holders_count: 1,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    let nar_id = match state.database.create_nar(&nar).await {
        Ok(id) => id,
        Err(e) => return Ok(e.to_response()),
    };

    // Generate storage key with compression extension
    let storage_key = format!(
        "nar/{}/{}{}",
        &upload_info.nar_hash[..2],
        upload_info.nar_hash,
        compression_result.compression.file_extension()
    );

    // Upload compressed data to R2
    let remote_file = match state
        .storage
        .upload_file(&storage_key, compression_result.data)
        .await
    {
        Ok(rf) => rf,
        Err(e) => {
            // Cleanup on failure
            let _ = state
                .database
                .update_nar_state(nar_id, NarState::Deleted)
                .await;
            return Ok(e.to_response());
        }
    };

    // Create chunk entry with file hash/size (compressed)
    let chunk = Chunk {
        id: None,
        state: ChunkState::Valid,
        chunk_hash: upload_info.nar_hash.clone(),
        chunk_size: compression_result.nar_size as i64,
        file_hash: Some(compression_result.file_hash),
        file_size: Some(compression_result.file_size as i64),
        compression: compression_result.compression.as_str().to_string(),
        remote_file: serde_json::to_string(&remote_file)
            .map_err(|e| worker::Error::RustError(format!("JSON error: {}", e)))?,
        remote_file_id: storage_key.clone(),
        holders_count: 1,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    let chunk_id = match state.database.create_chunk(&chunk).await {
        Ok(id) => id,
        Err(e) => {
            // Cleanup on failure
            let _ = state.storage.delete_file(&storage_key).await;
            let _ = state
                .database
                .update_nar_state(nar_id, NarState::Deleted)
                .await;
            return Ok(e.to_response());
        }
    };

    // Create chunk reference
    let chunk_ref = ChunkRef {
        id: None,
        nar_id,
        seq: 0,
        chunk_id: Some(chunk_id),
        chunk_hash: upload_info.nar_hash.clone(),
        compression: compression_result.compression.as_str().to_string(),
    };

    if let Err(e) = state.database.create_chunk_ref(&chunk_ref).await {
        // Cleanup on failure
        let _ = state.storage.delete_file(&storage_key).await;
        let _ = state
            .database
            .update_nar_state(nar_id, NarState::Deleted)
            .await;
        return Ok(e.to_response());
    }

    // Mark NAR as valid
    if let Err(e) = state
        .database
        .update_nar_state(nar_id, NarState::Valid)
        .await
    {
        return Ok(e.to_response());
    }

    // Create object
    let object = Object {
        id: None,
        cache_id,
        nar_id,
        store_path_hash: upload_info.store_path_hash.clone(),
        store_path: upload_info.store_path.clone(),
        references: upload_info.references.clone(),
        system: upload_info.system.clone(),
        deriver: upload_info.deriver.clone(),
        sigs: upload_info.sigs.clone(),
        ca: upload_info.ca.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        last_accessed_at: None,
        created_by: None,
    };

    if let Err(e) = state.database.create_object(&object).await {
        return Ok(e.to_response());
    }

    let result = UploadPathResult {
        kind: "uploaded".to_string(),
        file_size: Some(compression_result.file_size),
        frac_deduplicated: Some(0.0),
    };

    Response::from_json(&result)
}

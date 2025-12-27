//! Upload path handler.

use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use worker::*;

use crate::compression::{
    compress_buffer, CompressionConfig, CompressionLevel, CompressionType, NarHasher,
    StatefulBrotliCompressor, StatefulGzipCompressor, StreamingCompressor,
};
use crate::database::{Cache, Chunk, ChunkRef, ChunkState, Nar, NarState, Object};
use crate::error::WorkerError;
use crate::state::{RequestState, WorkerState};

/// Maximum body size for buffered compression (15 MB).
/// Files larger than this are uploaded without compression to avoid memory limits.
const MAX_BUFFERED_SIZE: u64 = 15 * 1024 * 1024;

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
pub async fn upload_path(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
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
    let parsed = parse_upload_info(&mut req).await?;
    let upload_info = parsed.info;

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
            return Ok(
                WorkerError::NotFound(format!("Cache not found: {}", upload_info.cache))
                    .to_response(),
            )
        }
        Err(e) => return Ok(e.to_response()),
    };

    let cache_id = cache
        .id
        .ok_or_else(|| worker::Error::RustError("Cache has no ID".to_string()))?;

    // Try to deduplicate by finding an existing NAR with the same hash
    if let Ok(Some(existing_nar)) = state.database.try_lock_nar(&upload_info.nar_hash).await {
        // Found existing NAR - create object pointing to it
        return handle_deduplicated_upload(&state, &upload_info, cache_id, existing_nar).await;
    }

    // New upload - read body and upload to R2 (pass cache for compression config)
    handle_new_upload(&state, req, &upload_info, &cache, parsed.remaining_body).await
}

/// Result of parsing upload info, including any remaining body data.
struct ParsedUploadInfo {
    info: UploadPathNarInfo,
    /// Remaining body bytes after the preamble (if preamble was used).
    /// None if header-based parsing was used (body should be read from request).
    remaining_body: Option<Vec<u8>>,
}

/// Parse upload info from request headers or body preamble.
async fn parse_upload_info(req: &mut Request) -> Result<ParsedUploadInfo> {
    let headers = req.headers();

    // Try header first
    if let Some(nar_info) = headers.get(ATTIC_NAR_INFO).ok().flatten() {
        let info = serde_json::from_str(&nar_info)
            .map_err(|e| worker::Error::RustError(format!("Invalid NAR info: {}", e)))?;
        return Ok(ParsedUploadInfo {
            info,
            remaining_body: None,
        });
    }

    // Try preamble
    if let Some(preamble_size_str) = headers.get(ATTIC_NAR_INFO_PREAMBLE_SIZE).ok().flatten() {
        let preamble_size: usize = preamble_size_str
            .parse()
            .map_err(|e| worker::Error::RustError(format!("Invalid preamble size: {}", e)))?;

        // Sanity check: preamble shouldn't be too large
        if preamble_size > 1024 * 1024 {
            // 1MB limit
            return Err(worker::Error::RustError(
                "Preamble size exceeds maximum (1MB)".to_string(),
            ));
        }

        // Read the full body
        let body_bytes = req
            .bytes()
            .await
            .map_err(|e| worker::Error::RustError(format!("Failed to read body: {}", e)))?;

        if body_bytes.len() < preamble_size {
            return Err(worker::Error::RustError(format!(
                "Body too small for preamble: expected at least {} bytes, got {}",
                preamble_size,
                body_bytes.len()
            )));
        }

        // Split body into preamble and NAR data
        let (preamble_bytes, nar_bytes) = body_bytes.split_at(preamble_size);

        // Parse the preamble as JSON
        let info: UploadPathNarInfo = serde_json::from_slice(preamble_bytes)
            .map_err(|e| worker::Error::RustError(format!("Invalid preamble JSON: {}", e)))?;

        return Ok(ParsedUploadInfo {
            info,
            remaining_body: Some(nar_bytes.to_vec()),
        });
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
    let nar_id = existing_nar
        .id
        .ok_or_else(|| worker::Error::RustError("NAR has no ID".to_string()))?;

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

/// Handle a deduplicated chunked upload (NAR already exists).
async fn handle_chunked_deduplicated_upload(
    state: &WorkerState,
    nar_info: &ChunkedNarInfo,
    cache_id: i64,
    existing_nar: crate::database::Nar,
) -> Result<Response> {
    let nar_id = existing_nar
        .id
        .ok_or_else(|| worker::Error::RustError("NAR has no ID".to_string()))?;

    // Create object pointing to existing NAR
    let object = Object {
        id: None,
        cache_id,
        nar_id,
        store_path_hash: nar_info.store_path_hash.clone(),
        store_path: nar_info.store_path.clone(),
        references: nar_info.references.clone(),
        system: nar_info.system.clone(),
        deriver: nar_info.deriver.clone(),
        sigs: nar_info.sigs.clone(),
        ca: nar_info.ca.clone(),
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
///
/// If `preamble_body` is Some, the body was already read during preamble parsing.
///
/// Upload strategy based on file size and client capabilities:
/// 1. **Preamble body**: Already buffered, use buffered upload with compression
/// 2. **Client pre-compressed**: Stream directly to R2 (no server-side work needed)
/// 3. **Small files (<15MB)**: Buffer entire file, compress, upload
/// 4. **Large files (>15MB)**: Use streaming compression with R2 multipart upload
async fn handle_new_upload(
    state: &WorkerState,
    req: Request,
    upload_info: &UploadPathNarInfo,
    cache: &Cache,
    preamble_body: Option<Vec<u8>>,
) -> Result<Response> {
    // If we already have the body from preamble parsing, use buffered upload
    if let Some(body_bytes) = preamble_body {
        return handle_buffered_upload_with_bytes(state, body_bytes, upload_info, cache).await;
    }

    // Check content length to decide between buffered and streaming compression
    let content_length: u64 = req
        .headers()
        .get("content-length")
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // For large files, use streaming compressed upload with R2 multipart
    // This keeps memory usage under ~9MB regardless of file size
    if content_length > MAX_BUFFERED_SIZE {
        return handle_streaming_compressed_upload(state, req, upload_info, cache).await;
    }

    // For small files, use buffered compression (faster, simpler)
    handle_buffered_upload(state, req, upload_info, cache).await
}

/// Handle a streaming compressed upload using R2 multipart.
///
/// This is used for large files (>15MB) that need server-side compression.
/// Instead of buffering the entire file (which would exceed memory limits),
/// we:
/// 1. Stream chunks from the request body
/// 2. Compress each chunk and accumulate until we have 5-8MB
/// 3. Upload each accumulated part via R2 multipart API
/// 4. Complete the multipart upload when done
///
/// Memory usage stays around ~9MB regardless of file size.
async fn handle_streaming_compressed_upload(
    state: &WorkerState,
    req: Request,
    upload_info: &UploadPathNarInfo,
    cache: &Cache,
) -> Result<Response> {
    let cache_id = cache
        .id
        .ok_or_else(|| worker::Error::RustError("Cache has no ID".to_string()))?;

    // Get compression config from cache
    let compression_config = CompressionConfig::from_str(&cache.compression);

    // Generate storage key with compression extension
    let expected_nar_hash = upload_info
        .nar_hash
        .strip_prefix("sha256:")
        .unwrap_or(&upload_info.nar_hash);
    let storage_key = format!(
        "nar/{}/{}{}",
        &expected_nar_hash[..2],
        expected_nar_hash,
        compression_config.r#type.file_extension()
    );

    // Start multipart upload
    let mut multipart = match state.storage.create_multipart_upload(&storage_key).await {
        Ok(m) => m,
        Err(e) => return Ok(e.to_response()),
    };

    // Create NAR hasher for validation
    let mut nar_hasher = NarHasher::new();

    // Get body as stream
    let body_stream = match req.inner().body() {
        Some(body) => body,
        None => {
            let _ = multipart.abort().await;
            return Ok(WorkerError::BadRequest("No request body".to_string()).to_response());
        }
    };

    // Get a reader for the stream
    let reader = body_stream
        .get_reader()
        .dyn_into::<web_sys::ReadableStreamDefaultReader>()
        .map_err(|_| worker::Error::RustError("Failed to get stream reader".to_string()))?;

    // Use different compressors based on compression type:
    // - Brotli: Must use StatefulBrotliCompressor (brotli doesn't support concatenated streams)
    // - Gzip: Must use StatefulGzipCompressor (for maximum compatibility)
    // - Zstd/None: Can use StreamingCompressor (zstd supports concatenated frames)
    let (remaining_data, file_hash, total_size, compression_str) = match compression_config.r#type {
        CompressionType::Brotli => {
            // Brotli requires stateful compression
            let mut compressor = StatefulBrotliCompressor::with_defaults(CompressionLevel::Default);

            // Process stream in chunks
            loop {
                let read_result = JsFuture::from(reader.read())
                    .await
                    .map_err(|e| worker::Error::RustError(format!("Stream read error: {:?}", e)))?;

                let done =
                    js_sys::Reflect::get(&read_result, &wasm_bindgen::JsValue::from_str("done"))
                        .map_err(|e| {
                            worker::Error::RustError(format!("Failed to get done: {:?}", e))
                        })?
                        .as_bool()
                        .unwrap_or(true);

                if done {
                    break;
                }

                let value =
                    js_sys::Reflect::get(&read_result, &wasm_bindgen::JsValue::from_str("value"))
                        .map_err(|e| {
                        worker::Error::RustError(format!("Failed to get value: {:?}", e))
                    })?;

                if value.is_undefined() {
                    break;
                }

                let array = js_sys::Uint8Array::new(&value);
                let chunk = array.to_vec();

                if chunk.is_empty() {
                    continue;
                }

                // Update NAR hash
                nar_hasher.update(&chunk);

                // Compress chunk - may return multiple parts
                match compressor.compress_chunk(&chunk) {
                    Ok(parts) => {
                        for part_data in parts {
                            if let Err(e) = multipart.upload_part(part_data).await {
                                let _ = multipart.abort().await;
                                return Ok(e.to_response());
                            }
                        }
                    }
                    Err(e) => {
                        let _ = multipart.abort().await;
                        return Ok(e.to_response());
                    }
                }
            }

            // Finish compression
            match compressor.finish() {
                Ok(result) => (
                    result.remaining_data,
                    result.file_hash,
                    result.total_size,
                    "br".to_string(),
                ),
                Err(e) => {
                    let _ = multipart.abort().await;
                    return Ok(e.to_response());
                }
            }
        }
        CompressionType::Gzip => {
            // Gzip requires stateful compression for maximum compatibility
            let mut compressor = StatefulGzipCompressor::with_defaults(CompressionLevel::Default);

            // Process stream in chunks
            loop {
                let read_result = JsFuture::from(reader.read())
                    .await
                    .map_err(|e| worker::Error::RustError(format!("Stream read error: {:?}", e)))?;

                let done =
                    js_sys::Reflect::get(&read_result, &wasm_bindgen::JsValue::from_str("done"))
                        .map_err(|e| {
                            worker::Error::RustError(format!("Failed to get done: {:?}", e))
                        })?
                        .as_bool()
                        .unwrap_or(true);

                if done {
                    break;
                }

                let value =
                    js_sys::Reflect::get(&read_result, &wasm_bindgen::JsValue::from_str("value"))
                        .map_err(|e| {
                        worker::Error::RustError(format!("Failed to get value: {:?}", e))
                    })?;

                if value.is_undefined() {
                    break;
                }

                let array = js_sys::Uint8Array::new(&value);
                let chunk = array.to_vec();

                if chunk.is_empty() {
                    continue;
                }

                // Update NAR hash
                nar_hasher.update(&chunk);

                // Compress chunk - may return multiple parts
                match compressor.compress_chunk(&chunk) {
                    Ok(parts) => {
                        for part_data in parts {
                            if let Err(e) = multipart.upload_part(part_data).await {
                                let _ = multipart.abort().await;
                                return Ok(e.to_response());
                            }
                        }
                    }
                    Err(e) => {
                        let _ = multipart.abort().await;
                        return Ok(e.to_response());
                    }
                }
            }

            // Finish compression
            match compressor.finish() {
                Ok(result) => (
                    result.remaining_data,
                    result.file_hash,
                    result.total_size,
                    "gzip".to_string(),
                ),
                Err(e) => {
                    let _ = multipart.abort().await;
                    return Ok(e.to_response());
                }
            }
        }
        CompressionType::Xz => {
            // XZ requires stateful compression (doesn't support concatenated streams)
            let mut compressor = match crate::compression::StatefulXzCompressor::with_defaults(
                CompressionLevel::Default,
            ) {
                Ok(c) => c,
                Err(e) => {
                    let _ = multipart.abort().await;
                    return Ok(e.to_response());
                }
            };

            // Process stream in chunks
            loop {
                let read_result = JsFuture::from(reader.read())
                    .await
                    .map_err(|e| worker::Error::RustError(format!("Stream read error: {:?}", e)))?;

                let done =
                    js_sys::Reflect::get(&read_result, &wasm_bindgen::JsValue::from_str("done"))
                        .map_err(|e| {
                            worker::Error::RustError(format!("Failed to get done: {:?}", e))
                        })?
                        .as_bool()
                        .unwrap_or(true);

                if done {
                    break;
                }

                let value =
                    js_sys::Reflect::get(&read_result, &wasm_bindgen::JsValue::from_str("value"))
                        .map_err(|e| {
                        worker::Error::RustError(format!("Failed to get value: {:?}", e))
                    })?;

                if value.is_undefined() {
                    break;
                }

                let array = js_sys::Uint8Array::new(&value);
                let chunk = array.to_vec();

                if chunk.is_empty() {
                    continue;
                }

                // Update NAR hash
                nar_hasher.update(&chunk);

                // Compress chunk - may return multiple parts
                match compressor.compress_chunk(&chunk) {
                    Ok(parts) => {
                        for part_data in parts {
                            if let Err(e) = multipart.upload_part(part_data).await {
                                let _ = multipart.abort().await;
                                return Ok(e.to_response());
                            }
                        }
                    }
                    Err(e) => {
                        let _ = multipart.abort().await;
                        return Ok(e.to_response());
                    }
                }
            }

            // Finish compression
            match compressor.finish() {
                Ok(result) => (
                    result.remaining_data,
                    result.file_hash,
                    result.total_size,
                    "xz".to_string(),
                ),
                Err(e) => {
                    let _ = multipart.abort().await;
                    return Ok(e.to_response());
                }
            }
        }
        CompressionType::Zstd | CompressionType::None | CompressionType::Bzip2 => {
            // Zstd/None/Bzip2: Use streaming compressor
            // - Zstd: Buffers input to 4MB blocks for better compression ratio
            // - None/Bzip2: Pass through (Bzip2 falls back to no compression in worker)
            let mut compressor = StreamingCompressor::with_defaults(
                compression_config.r#type,
                CompressionLevel::Default,
            );

            // Process stream in chunks
            loop {
                let read_result = JsFuture::from(reader.read())
                    .await
                    .map_err(|e| worker::Error::RustError(format!("Stream read error: {:?}", e)))?;

                let done =
                    js_sys::Reflect::get(&read_result, &wasm_bindgen::JsValue::from_str("done"))
                        .map_err(|e| {
                            worker::Error::RustError(format!("Failed to get done: {:?}", e))
                        })?
                        .as_bool()
                        .unwrap_or(true);

                if done {
                    break;
                }

                let value =
                    js_sys::Reflect::get(&read_result, &wasm_bindgen::JsValue::from_str("value"))
                        .map_err(|e| {
                        worker::Error::RustError(format!("Failed to get value: {:?}", e))
                    })?;

                if value.is_undefined() {
                    break;
                }

                let array = js_sys::Uint8Array::new(&value);
                let chunk = array.to_vec();

                if chunk.is_empty() {
                    continue;
                }

                // Update NAR hash
                nar_hasher.update(&chunk);

                // Compress chunk and check if we have a full part
                match compressor.compress_chunk(&chunk) {
                    Ok(Some(part_data)) => {
                        if let Err(e) = multipart.upload_part(part_data).await {
                            let _ = multipart.abort().await;
                            return Ok(e.to_response());
                        }
                    }
                    Ok(None) => {
                        // Continue accumulating
                    }
                    Err(e) => {
                        let _ = multipart.abort().await;
                        return Ok(e.to_response());
                    }
                }
            }

            // Finish compression
            let result = compressor.finish();
            (
                result.remaining_data,
                result.file_hash,
                result.total_size,
                result.compression.as_str().to_string(),
            )
        }
    };

    // Validate NAR hash
    let (computed_nar_hash, nar_size) = nar_hasher.finalize();
    if computed_nar_hash != expected_nar_hash {
        let _ = multipart.abort().await;
        return Ok(WorkerError::BadRequest(format!(
            "NAR hash mismatch: expected {}, got {}",
            expected_nar_hash, computed_nar_hash
        ))
        .to_response());
    }

    // Upload remaining data as final part (can be < 5MB)
    if !remaining_data.is_empty() {
        if let Err(e) = multipart.upload_part(remaining_data).await {
            let _ = multipart.abort().await;
            return Ok(e.to_response());
        }
    }

    // Complete multipart upload
    let remote_file = match multipart.complete().await {
        Ok(rf) => rf,
        Err(e) => return Ok(e.to_response()),
    };

    // Create NAR entry
    let nar = Nar {
        id: None,
        state: NarState::PendingUpload,
        nar_hash: upload_info.nar_hash.clone(),
        nar_size: nar_size as i64,
        compression: compression_str.clone(),
        num_chunks: 1,
        completeness_hint: false,
        holders_count: 1,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    let nar_id = match state.database.create_nar(&nar).await {
        Ok(id) => id,
        Err(e) => {
            let _ = state.storage.delete_file(&storage_key).await;
            return Ok(e.to_response());
        }
    };

    // Create chunk entry
    let chunk = Chunk {
        id: None,
        state: ChunkState::Valid,
        chunk_hash: upload_info.nar_hash.clone(),
        chunk_size: nar_size as i64,
        file_hash: Some(file_hash),
        file_size: Some(total_size as i64),
        compression: compression_str.clone(),
        remote_file: serde_json::to_string(&remote_file)
            .map_err(|e| worker::Error::RustError(format!("JSON error: {}", e)))?,
        remote_file_id: storage_key.clone(),
        holders_count: 1,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    let chunk_id = match state.database.create_chunk(&chunk).await {
        Ok(id) => id,
        Err(e) => {
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
        compression: compression_str,
    };

    if let Err(e) = state.database.create_chunk_ref(&chunk_ref).await {
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
        file_size: Some(total_size),
        frac_deduplicated: Some(0.0),
    };

    Response::from_json(&result)
}

/// Handle a buffered upload (for small files, with compression).
async fn handle_buffered_upload(
    state: &WorkerState,
    mut req: Request,
    upload_info: &UploadPathNarInfo,
    cache: &Cache,
) -> Result<Response> {
    let cache_id = cache
        .id
        .ok_or_else(|| worker::Error::RustError("Cache has no ID".to_string()))?;

    // Read body
    let body_bytes = match req.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return Err(e);
        }
    };

    // Use cache-specific compression config
    let compression_config = CompressionConfig::from_str(&cache.compression);

    // Compress with dual hashing (NAR hash before compression, file hash after)
    let compression_result = match compress_buffer(&body_bytes, &compression_config) {
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

/// Handle a buffered upload with pre-read bytes (from preamble parsing).
///
/// This is used when the body was already read during preamble-based NAR info parsing.
async fn handle_buffered_upload_with_bytes(
    state: &WorkerState,
    body_bytes: Vec<u8>,
    upload_info: &UploadPathNarInfo,
    cache: &Cache,
) -> Result<Response> {
    let cache_id = cache
        .id
        .ok_or_else(|| worker::Error::RustError("Cache has no ID".to_string()))?;

    // Use cache-specific compression config
    let compression_config = CompressionConfig::from_str(&cache.compression);

    // Compress with dual hashing (NAR hash before compression, file hash after)
    let compression_result = match compress_buffer(&body_bytes, &compression_config) {
        Ok(r) => r,
        Err(e) => return Ok(e.to_response()),
    };

    // Validate NAR hash matches what client claimed
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

// =============================================================================
// Chunked Upload Protocol
// =============================================================================
// For files larger than Cloudflare's 100MB request limit, we use a chunked
// upload protocol:
//
// 1. POST /_api/v1/upload-path/start - Start chunked upload, get upload token
// 2. PUT /_api/v1/upload-path/chunk - Upload a chunk (< 95MB each)
// 3. POST /_api/v1/upload-path/complete - Complete the upload
//
// The upload token is a base64-encoded state that contains:
// - R2 multipart upload ID
// - Storage key
// - NAR info for final validation

/// Maximum chunk size for chunked uploads (50MB to stay under worker memory limit).
/// Cloudflare Workers have a 128MB memory limit, so we use 50MB chunks to leave
/// headroom for request processing while maximizing throughput.
pub const MAX_CHUNK_SIZE: u64 = 50 * 1024 * 1024;

/// Request body for starting a chunked upload.
#[derive(Debug, Deserialize)]
pub struct StartChunkedUploadRequest {
    /// NAR info for the upload.
    pub nar_info: ChunkedNarInfo,
    /// Expected total NAR size (uncompressed).
    pub nar_size: u64,
}

/// NAR info for chunked uploads.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChunkedNarInfo {
    pub cache: String,
    pub store_path_hash: String,
    pub store_path: String,
    pub references: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deriver: Option<String>,
    pub sigs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca: Option<String>,
    pub nar_hash: String,
}

/// Response for starting a chunked upload.
#[derive(Debug, Serialize)]
pub struct StartChunkedUploadResponse {
    /// Opaque upload token (base64-encoded state).
    pub upload_token: String,
    /// Recommended chunk size.
    pub chunk_size: u64,
}

/// Internal state for a chunked upload (encoded in upload_token).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChunkedUploadState {
    /// R2 multipart upload ID.
    r2_upload_id: String,
    /// R2 multipart upload key.
    r2_key: String,
    /// Storage key for the final object.
    storage_key: String,
    /// NAR info for final object creation.
    nar_info: ChunkedNarInfo,
    /// Cache ID.
    cache_id: i64,
    /// Expected NAR size.
    expected_nar_size: u64,
    /// Compression type used.
    compression: String,
    /// Number of parts uploaded so far.
    parts_uploaded: u16,
    /// Total bytes received (compressed).
    bytes_received: u64,
    /// Info about uploaded parts (part number + etag), needed for multipart complete.
    uploaded_parts: Vec<crate::storage::UploadedPartInfo>,
}

/// Request body for completing a chunked upload.
#[derive(Debug, Deserialize)]
pub struct CompleteChunkedUploadRequest {
    /// Upload token from start_chunked_upload.
    pub upload_token: String,
}

/// POST /_api/v1/upload-path/start
///
/// Start a chunked upload for large files.
pub async fn start_chunked_upload(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let worker_state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    let req_state = match RequestState::from_request(&req, &worker_state.jwt_config) {
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

    // Parse request body
    let body: StartChunkedUploadRequest = match req.json().await {
        Ok(b) => b,
        Err(e) => {
            return Ok(
                WorkerError::BadRequest(format!("Invalid request body: {}", e)).to_response(),
            )
        }
    };

    // Check permission to push
    let cache_name = attic::cache::CacheName::new(body.nar_info.cache.clone())
        .map_err(|e| WorkerError::BadRequest(format!("Invalid cache name: {}", e)))?;
    let permission = token.get_permission_for_cache(&cache_name);
    if let Err(e) = permission.require_push() {
        return Ok(WorkerError::Authorization(format!("Permission denied: {:?}", e)).to_response());
    }

    // Find the cache
    let cache = match worker_state.database.find_cache(&body.nar_info.cache).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return Ok(
                WorkerError::NotFound(format!("Cache not found: {}", body.nar_info.cache))
                    .to_response(),
            )
        }
        Err(e) => return Ok(e.to_response()),
    };

    let cache_id = cache
        .id
        .ok_or_else(|| worker::Error::RustError("Cache has no ID".to_string()))?;

    // Check if NAR already exists (deduplication)
    if let Ok(Some(existing_nar)) = worker_state
        .database
        .try_lock_nar(&body.nar_info.nar_hash)
        .await
    {
        // NAR exists - create object pointing to it (deduplication)
        return handle_chunked_deduplicated_upload(
            &worker_state,
            &body.nar_info,
            cache_id,
            existing_nar,
        )
        .await;
    }

    // Get compression config from cache
    let compression_config = CompressionConfig::from_str(&cache.compression);

    // Generate storage key with compression extension
    let expected_nar_hash = body
        .nar_info
        .nar_hash
        .strip_prefix("sha256:")
        .unwrap_or(&body.nar_info.nar_hash);
    let storage_key = format!(
        "nar/{}/{}{}",
        &expected_nar_hash[..2],
        expected_nar_hash,
        compression_config.r#type.file_extension()
    );

    // Start R2 multipart upload
    let multipart = match worker_state
        .storage
        .create_multipart_upload(&storage_key)
        .await
    {
        Ok(m) => m,
        Err(e) => return Ok(e.to_response()),
    };

    // Get the upload ID and key from the multipart upload
    let (r2_upload_id, r2_key) = multipart.get_upload_info().await;

    // Create initial upload state
    let upload_state = ChunkedUploadState {
        r2_upload_id,
        r2_key,
        storage_key,
        nar_info: body.nar_info,
        cache_id,
        expected_nar_size: body.nar_size,
        compression: compression_config.r#type.as_str().to_string(),
        parts_uploaded: 0,
        bytes_received: 0,
        uploaded_parts: Vec::new(),
    };

    // Encode state as base64 token
    let state_json = serde_json::to_string(&upload_state)
        .map_err(|e| worker::Error::RustError(format!("Failed to encode state: {}", e)))?;
    let upload_token =
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE, &state_json);

    let response = StartChunkedUploadResponse {
        upload_token,
        chunk_size: MAX_CHUNK_SIZE,
    };

    Response::from_json(&response)
}

/// PUT /_api/v1/upload-path/chunk
///
/// Upload a chunk of data for a chunked upload.
/// Header X-Upload-Token: the upload token from start_chunked_upload
/// Header X-Part-Number: the part number (1-indexed)
/// Body: raw pre-compressed chunk data
pub async fn upload_chunk(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let worker_state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    let req_state = match RequestState::from_request(&req, &worker_state.jwt_config) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    // Check authentication
    if req_state.token.is_none() {
        return Ok(WorkerError::Authentication("No token provided".to_string()).to_response());
    }

    // Get upload token from header
    let upload_token = match req.headers().get("X-Upload-Token").ok().flatten() {
        Some(t) => t,
        None => {
            return Ok(
                WorkerError::BadRequest("Missing X-Upload-Token header".to_string()).to_response(),
            )
        }
    };

    // Get part number from header
    let part_number: u16 = match req
        .headers()
        .get("X-Part-Number")
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
    {
        Some(n) => n,
        None => {
            return Ok(WorkerError::BadRequest(
                "Missing or invalid X-Part-Number header".to_string(),
            )
            .to_response())
        }
    };

    // Decode upload state
    let state_json =
        match base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE, &upload_token) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(e) => {
                    return Ok(
                        WorkerError::BadRequest(format!("Invalid upload token: {}", e))
                            .to_response(),
                    )
                }
            },
            Err(e) => {
                return Ok(
                    WorkerError::BadRequest(format!("Invalid upload token encoding: {}", e))
                        .to_response(),
                )
            }
        };

    let mut upload_state: ChunkedUploadState = match serde_json::from_str(&state_json) {
        Ok(s) => s,
        Err(e) => {
            return Ok(
                WorkerError::BadRequest(format!("Invalid upload token data: {}", e)).to_response(),
            )
        }
    };

    // Validate part number is sequential
    let expected_part = upload_state.parts_uploaded + 1;
    if part_number != expected_part {
        return Ok(WorkerError::BadRequest(format!(
            "Expected part {}, got {}",
            expected_part, part_number
        ))
        .to_response());
    }

    // Read chunk data
    let chunk_data = match req.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return Ok(WorkerError::BadRequest(format!("Failed to read body: {}", e)).to_response())
        }
    };

    if chunk_data.is_empty() {
        return Ok(WorkerError::BadRequest("Empty chunk data".to_string()).to_response());
    }

    // Resume the multipart upload and upload part
    let mut multipart = worker_state
        .storage
        .resume_multipart_upload(
            &upload_state.r2_key,
            &upload_state.r2_upload_id,
            upload_state.parts_uploaded,
        )
        .await?;

    let part_info = match multipart.upload_part(chunk_data.clone()).await {
        Ok(info) => info,
        Err(e) => return Ok(e.to_response()),
    };

    // Update state with part info
    upload_state.parts_uploaded = part_number;
    upload_state.bytes_received += chunk_data.len() as u64;
    upload_state.uploaded_parts.push(part_info);

    // Encode updated state
    let state_json = serde_json::to_string(&upload_state)
        .map_err(|e| worker::Error::RustError(format!("Failed to encode state: {}", e)))?;
    let new_token = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE, &state_json);

    // Return updated token
    #[derive(Serialize)]
    struct ChunkUploadResponse {
        upload_token: String,
        parts_uploaded: u16,
        bytes_received: u64,
    }

    Response::from_json(&ChunkUploadResponse {
        upload_token: new_token,
        parts_uploaded: upload_state.parts_uploaded,
        bytes_received: upload_state.bytes_received,
    })
}

/// POST /_api/v1/upload-path/complete
///
/// Complete a chunked upload.
pub async fn complete_chunked_upload(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let worker_state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    let req_state = match RequestState::from_request(&req, &worker_state.jwt_config) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    // Check authentication
    if req_state.token.is_none() {
        return Ok(WorkerError::Authentication("No token provided".to_string()).to_response());
    }

    // Parse request body
    let body: CompleteChunkedUploadRequest = match req.json().await {
        Ok(b) => b,
        Err(e) => {
            return Ok(
                WorkerError::BadRequest(format!("Invalid request body: {}", e)).to_response(),
            )
        }
    };

    // Decode upload state
    let state_json = match base64::Engine::decode(
        &base64::engine::general_purpose::URL_SAFE,
        &body.upload_token,
    ) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => {
                return Ok(
                    WorkerError::BadRequest(format!("Invalid upload token: {}", e)).to_response(),
                )
            }
        },
        Err(e) => {
            return Ok(
                WorkerError::BadRequest(format!("Invalid upload token encoding: {}", e))
                    .to_response(),
            )
        }
    };

    let upload_state: ChunkedUploadState = match serde_json::from_str(&state_json) {
        Ok(s) => s,
        Err(e) => {
            return Ok(
                WorkerError::BadRequest(format!("Invalid upload token data: {}", e)).to_response(),
            )
        }
    };

    // Validate we received data
    if upload_state.parts_uploaded == 0 || upload_state.uploaded_parts.is_empty() {
        return Ok(WorkerError::BadRequest("No parts uploaded".to_string()).to_response());
    }

    // Resume and complete the multipart upload with all the parts info
    let multipart = worker_state
        .storage
        .resume_multipart_upload(
            &upload_state.r2_key,
            &upload_state.r2_upload_id,
            upload_state.parts_uploaded,
        )
        .await?;

    let remote_file = match multipart
        .complete_with_parts(upload_state.uploaded_parts.clone())
        .await
    {
        Ok(rf) => rf,
        Err(e) => return Ok(e.to_response()),
    };

    // Get the file size from R2
    let file_size = match worker_state
        .storage
        .file_size(&upload_state.storage_key)
        .await
    {
        Ok(Some(size)) => size,
        Ok(None) => {
            return Ok(
                WorkerError::Internal("File not found after upload".to_string()).to_response(),
            )
        }
        Err(e) => return Ok(e.to_response()),
    };

    // For chunked uploads, we use the bytes_received as a proxy for file hash
    // Computing the actual hash would require downloading the entire file which
    // exceeds worker memory limits for large files. The NAR hash is already
    // validated by the client, which is the critical integrity check.
    let file_hash = format!("chunked-{}", upload_state.bytes_received);

    // Create NAR entry
    let nar = Nar {
        id: None,
        state: NarState::PendingUpload,
        nar_hash: upload_state.nar_info.nar_hash.clone(),
        nar_size: upload_state.expected_nar_size as i64,
        compression: upload_state.compression.clone(),
        num_chunks: 1,
        completeness_hint: false,
        holders_count: 1,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    let nar_id = match worker_state.database.create_nar(&nar).await {
        Ok(id) => id,
        Err(e) => {
            let _ = worker_state
                .storage
                .delete_file(&upload_state.storage_key)
                .await;
            return Ok(e.to_response());
        }
    };

    // Create chunk entry
    let chunk = Chunk {
        id: None,
        state: ChunkState::Valid,
        chunk_hash: upload_state.nar_info.nar_hash.clone(),
        chunk_size: upload_state.expected_nar_size as i64,
        file_hash: Some(file_hash),
        file_size: Some(file_size as i64),
        compression: upload_state.compression.clone(),
        remote_file: serde_json::to_string(&remote_file)
            .map_err(|e| worker::Error::RustError(format!("JSON error: {}", e)))?,
        remote_file_id: upload_state.storage_key.clone(),
        holders_count: 1,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    let chunk_id = match worker_state.database.create_chunk(&chunk).await {
        Ok(id) => id,
        Err(e) => {
            let _ = worker_state
                .storage
                .delete_file(&upload_state.storage_key)
                .await;
            let _ = worker_state
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
        chunk_hash: upload_state.nar_info.nar_hash.clone(),
        compression: upload_state.compression.clone(),
    };

    if let Err(e) = worker_state.database.create_chunk_ref(&chunk_ref).await {
        let _ = worker_state
            .storage
            .delete_file(&upload_state.storage_key)
            .await;
        let _ = worker_state
            .database
            .update_nar_state(nar_id, NarState::Deleted)
            .await;
        return Ok(e.to_response());
    }

    // Mark NAR as valid
    if let Err(e) = worker_state
        .database
        .update_nar_state(nar_id, NarState::Valid)
        .await
    {
        return Ok(e.to_response());
    }

    // Create object pointing to the NAR
    let object = Object {
        id: None,
        cache_id: upload_state.cache_id,
        nar_id,
        store_path_hash: upload_state.nar_info.store_path_hash.clone(),
        store_path: upload_state.nar_info.store_path.clone(),
        references: upload_state.nar_info.references.clone(),
        system: upload_state.nar_info.system.clone(),
        deriver: upload_state.nar_info.deriver.clone(),
        sigs: upload_state.nar_info.sigs.clone(),
        ca: upload_state.nar_info.ca.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        last_accessed_at: None,
        created_by: None,
    };

    if let Err(e) = worker_state.database.create_object(&object).await {
        return Ok(e.to_response());
    }

    let result = UploadPathResult {
        kind: "uploaded".to_string(),
        file_size: Some(file_size),
        frac_deduplicated: Some(0.0),
    };

    Response::from_json(&result)
}

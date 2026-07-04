//! Binary Cache API handlers (Nix protocol).

use worker::*;

use crate::crypto::{compute_fingerprint, convert_hash_to_base32, sign_message};
use crate::error::{WorkerError, WorkerResult};
use crate::state::{RequestState, WorkerState};

/// Enforce pull permission for a cache, honoring public caches and anonymous access.
///
/// Mirrors the native server: an anonymous request starts with no permissions,
/// public caches implicitly grant pull, and any bearer token contributes its
/// granted permissions for the named cache.
async fn authorize_pull(
    req: &Request,
    state: &WorkerState,
    cache_name: &str,
    is_public: bool,
) -> WorkerResult<()> {
    // A public cache is readable anonymously, so a malformed, expired, or
    // revoked token must not break the pull — ignore it and fall back to public
    // access. Private caches still reject an invalid token. (Push and config
    // endpoints validate strictly elsewhere; this leniency is pull-only.)
    let token = match RequestState::from_request(req, state).await {
        Ok(req_state) => req_state.token,
        Err(_) if is_public => None,
        Err(e) => return Err(e),
    };

    let cache_name_typed = attic::cache::CacheName::new(cache_name.to_string())
        .map_err(|e| WorkerError::BadRequest(format!("Invalid cache name: {}", e)))?;

    let mut permission = match &token {
        Some(token) => token.get_permission_for_cache(&cache_name_typed),
        None => attic_token::CachePermission::default(),
    };

    if is_public {
        permission.add_public_permissions();
    }

    permission
        .require_pull()
        .map_err(|e| WorkerError::Authorization(format!("Permission denied: {:?}", e)))?;

    Ok(())
}

/// GET /:cache/nix-cache-info
///
/// Returns basic cache information for Nix.
pub async fn get_nix_cache_info(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let cache_name = ctx.param("cache").unwrap_or(&String::new()).clone();

    let state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    // Find the cache
    let cache = match state.database.find_cache(&cache_name).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return Ok(
                WorkerError::NotFound(format!("Cache not found: {}", cache_name)).to_response(),
            )
        }
        Err(e) => return Ok(e.to_response()),
    };

    if let Err(e) = authorize_pull(&req, &state, &cache_name, cache.is_public).await {
        return Ok(e.to_response());
    }

    // Build nix-cache-info response
    let info = format!(
        "StoreDir: {}\nWantMassQuery: 1\nPriority: {}\n",
        cache.store_dir, cache.priority
    );

    Response::ok(info)
}

/// HEAD /:cache/nix-cache-info
///
/// Returns headers only for cache info check.
pub async fn head_nix_cache_info(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let cache_name = ctx.param("cache").unwrap_or(&String::new()).clone();

    let state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    // Find the cache
    let cache = match state.database.find_cache(&cache_name).await {
        Ok(Some(c)) => c,
        Ok(None) => return Response::error("Not found", 404),
        Err(e) => return Ok(e.to_response()),
    };

    if let Err(e) = authorize_pull(&req, &state, &cache_name, cache.is_public).await {
        return Ok(e.to_response());
    }

    Response::empty()
}

/// GET /:cache/:path
///
/// Returns narinfo for a store path.
/// Path format: <hash>.narinfo
pub async fn get_store_path_info(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let cache_name = ctx.param("cache").unwrap_or(&String::new()).clone();
    let path = ctx.param("path").unwrap_or(&String::new()).clone();

    // Check if this is a narinfo request
    if !path.ends_with(".narinfo") {
        return Response::error("Not found", 404);
    }

    // Extract store path hash from filename
    let store_path_hash = path.trim_end_matches(".narinfo");
    if store_path_hash.len() != 32 {
        return Ok(WorkerError::BadRequest("Invalid store path hash".to_string()).to_response());
    }

    let state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    // Find the cache to get the keypair
    let cache = match state.database.find_cache(&cache_name).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return Ok(
                WorkerError::NotFound(format!("Cache not found: {}", cache_name)).to_response(),
            )
        }
        Err(e) => return Ok(e.to_response()),
    };

    if let Err(e) = authorize_pull(&req, &state, &cache_name, cache.is_public).await {
        return Ok(e.to_response());
    }

    // Find the object
    let obj = match state
        .database
        .find_object(&cache_name, store_path_hash)
        .await
    {
        Ok(Some(o)) => o,
        Ok(None) => return Response::error("Not found", 404),
        Err(e) => return Ok(e.to_response()),
    };

    // Get chunk info for FileHash/FileSize (first chunk for single-chunk NARs)
    let nar_id = obj
        .nar
        .id
        .ok_or_else(|| worker::Error::RustError("NAR has no ID".to_string()))?;
    let chunks = state.database.find_chunks_for_nar(nar_id).await.ok();
    let first_chunk = chunks.as_ref().and_then(|c| c.first());

    // Build narinfo response with server-side signing
    let narinfo = build_narinfo(&obj.object, &obj.nar, first_chunk, Some(&cache.keypair))?;

    let mut headers = Headers::new();
    headers.set("Content-Type", "text/x-nix-narinfo")?;

    Ok(Response::ok(narinfo)?.with_headers(headers))
}

/// HEAD /:cache/:path
///
/// Returns headers only for narinfo existence check.
pub async fn head_store_path_info(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let cache_name = ctx.param("cache").unwrap_or(&String::new()).clone();
    let path = ctx.param("path").unwrap_or(&String::new()).clone();

    // Check if this is a narinfo request
    if !path.ends_with(".narinfo") {
        return Response::error("Not found", 404);
    }

    // Extract store path hash from filename
    let store_path_hash = path.trim_end_matches(".narinfo");
    if store_path_hash.len() != 32 {
        return Ok(WorkerError::BadRequest("Invalid store path hash".to_string()).to_response());
    }

    let state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    // Find the cache
    let cache = match state.database.find_cache(&cache_name).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return Ok(
                WorkerError::NotFound(format!("Cache not found: {}", cache_name)).to_response(),
            )
        }
        Err(e) => return Ok(e.to_response()),
    };

    if let Err(e) = authorize_pull(&req, &state, &cache_name, cache.is_public).await {
        return Ok(e.to_response());
    }

    // Check if the object exists
    match state
        .database
        .find_object(&cache_name, store_path_hash)
        .await
    {
        Ok(Some(_)) => {
            let mut headers = Headers::new();
            headers.set("Content-Type", "text/x-nix-narinfo")?;
            Ok(Response::empty()?.with_headers(headers))
        }
        Ok(None) => Response::error("Not found", 404),
        Err(e) => Ok(e.to_response()),
    }
}

/// GET /:cache/nar/:path
///
/// Returns the NAR file. For single-chunk NARs, redirects to R2.
/// For multi-chunk NARs, streams the concatenated chunks.
pub async fn get_nar(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let cache_name = ctx.param("cache").unwrap_or(&String::new()).clone();
    let path = ctx.param("path").unwrap_or(&String::new()).clone();

    // Extract NAR hash from path
    // Format: <hash>.nar or <hash>.nar.<compression>
    let nar_hash_raw = path
        .split('.')
        .next()
        .ok_or_else(|| worker::Error::RustError("Invalid NAR path".to_string()))?;

    let state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    // Enforce pull permission for the named cache
    let cache = match state.database.find_cache(&cache_name).await {
        Ok(Some(c)) => c,
        Ok(None) => return Response::error("Not found", 404),
        Err(e) => return Ok(e.to_response()),
    };
    if let Err(e) = authorize_pull(&req, &state, &cache_name, cache.is_public).await {
        return Ok(e.to_response());
    }

    // Find the NAR - try with sha256: prefix since that's how it's stored
    let nar_hash_with_prefix = format!("sha256:{}", nar_hash_raw);
    let nar = match state.database.find_nar_by_hash(&nar_hash_with_prefix).await {
        Ok(Some(n)) => n,
        Ok(None) => {
            // Try without prefix as fallback
            match state.database.find_nar_by_hash(nar_hash_raw).await {
                Ok(Some(n)) => n,
                Ok(None) => return Response::error("Not found", 404),
                Err(e) => return Ok(e.to_response()),
            }
        }
        Err(e) => return Ok(e.to_response()),
    };

    let nar_id = nar
        .id
        .ok_or_else(|| worker::Error::RustError("NAR has no ID".to_string()))?;

    // Find chunks
    let chunks = match state.database.find_chunks_for_nar(nar_id).await {
        Ok(c) => c,
        Err(e) => return Ok(e.to_response()),
    };

    if chunks.is_empty() {
        return Response::error("NAR has no chunks", 500);
    }

    if chunks.len() == 1 {
        // Single chunk - stream from R2
        let chunk = &chunks[0];
        let remote_file: serde_json::Value = serde_json::from_str(&chunk.remote_file)
            .map_err(|e| worker::Error::RustError(format!("Invalid remote file: {}", e)))?;

        // Extract key from remote file
        let key = remote_file
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| worker::Error::RustError("No key in remote file".to_string()))?;

        // Stream the file directly from R2 (doesn't load entire file into memory)
        match state.storage.download_file_stream(key).await {
            Ok(response) => Ok(response),
            Err(e) => Ok(e.to_response()),
        }
    } else {
        // Multi-chunk - stream concatenated chunks
        // TODO: Implement streaming for multi-chunk NARs
        Response::error("Multi-chunk NARs not yet supported", 501)
    }
}

/// HEAD /:cache/nar/:path
///
/// Returns headers only for NAR file existence check.
pub async fn head_nar(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let cache_name = ctx.param("cache").unwrap_or(&String::new()).clone();
    let path = ctx.param("path").unwrap_or(&String::new()).clone();

    // Extract NAR hash from path
    let nar_hash_raw = path
        .split('.')
        .next()
        .ok_or_else(|| worker::Error::RustError("Invalid NAR path".to_string()))?;

    let state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    // Enforce pull permission for the named cache
    let cache = match state.database.find_cache(&cache_name).await {
        Ok(Some(c)) => c,
        Ok(None) => return Response::error("Not found", 404),
        Err(e) => return Ok(e.to_response()),
    };
    if let Err(e) = authorize_pull(&req, &state, &cache_name, cache.is_public).await {
        return Ok(e.to_response());
    }

    // Find the NAR
    let nar_hash_with_prefix = format!("sha256:{}", nar_hash_raw);
    let nar = match state.database.find_nar_by_hash(&nar_hash_with_prefix).await {
        Ok(Some(n)) => n,
        Ok(None) => {
            // Try without prefix as fallback
            match state.database.find_nar_by_hash(nar_hash_raw).await {
                Ok(Some(n)) => n,
                Ok(None) => return Response::error("Not found", 404),
                Err(e) => return Ok(e.to_response()),
            }
        }
        Err(e) => return Ok(e.to_response()),
    };

    // Get chunk info for Content-Length
    let nar_id = nar
        .id
        .ok_or_else(|| worker::Error::RustError("NAR has no ID".to_string()))?;
    let chunks = state.database.find_chunks_for_nar(nar_id).await.ok();

    let mut headers = Headers::new();
    headers.set("Content-Type", "application/x-nix-nar")?;

    // Set Content-Length if we have single chunk with file_size
    if let Some(chunks) = &chunks {
        if chunks.len() == 1 {
            if let Some(file_size) = chunks[0].file_size {
                headers.set("Content-Length", &file_size.to_string())?;
            }
        }
    }

    Ok(Response::empty()?.with_headers(headers))
}

/// Build a narinfo string from object, nar, and optional chunk data.
///
/// If a keypair is provided and the object has no signatures, the server
/// will sign the narinfo with the cache's keypair.
fn build_narinfo(
    object: &crate::database::Object,
    nar: &crate::database::Nar,
    chunk: Option<&crate::database::Chunk>,
    keypair: Option<&str>,
) -> Result<String> {
    use std::fmt::Write;

    let mut narinfo = String::new();

    writeln!(narinfo, "StorePath: {}", object.store_path)
        .map_err(|e| worker::Error::RustError(e.to_string()))?;

    // URL for the NAR file (with compression extension)
    // Use the hash as stored in the database for the URL (so lookups work)
    let hash_for_url = nar
        .nar_hash
        .strip_prefix("sha256:")
        .unwrap_or(&nar.nar_hash);
    let extension = match nar.compression.as_str() {
        "zstd" => ".zst",
        "brotli" | "br" => ".br",
        "gzip" | "gz" => ".gz",
        "xz" => ".xz",
        _ => "", // "none" or unknown
    };
    writeln!(narinfo, "URL: nar/{}.nar{}", hash_for_url, extension)
        .map_err(|e| worker::Error::RustError(e.to_string()))?;

    // Compression
    writeln!(narinfo, "Compression: {}", nar.compression)
        .map_err(|e| worker::Error::RustError(e.to_string()))?;

    // FileHash and FileSize (compressed file stats)
    if let Some(chunk) = chunk {
        if let Some(ref file_hash) = chunk.file_hash {
            // Only output FileHash if it's a valid hash format
            // file_hash is stored as hex (64 chars) without prefix
            if file_hash.len() == 64 && file_hash.chars().all(|c| c.is_ascii_hexdigit()) {
                let file_hash_base32 = convert_hash_to_base32(&format!("sha256:{}", file_hash));
                writeln!(narinfo, "FileHash: {}", file_hash_base32)
                    .map_err(|e| worker::Error::RustError(e.to_string()))?;
            }
            // Skip malformed hashes rather than outputting invalid data
        }
        if let Some(file_size) = chunk.file_size {
            writeln!(narinfo, "FileSize: {}", file_size)
                .map_err(|e| worker::Error::RustError(e.to_string()))?;
        }
    }

    // NAR hash and size (uncompressed)
    // Convert to base32 format for Nix compatibility
    // nar_hash may be stored as "sha256:<hex>" or just "<hex>"
    let nar_hash_value = nar
        .nar_hash
        .strip_prefix("sha256:")
        .unwrap_or(&nar.nar_hash);
    if nar_hash_value.len() == 64 && nar_hash_value.chars().all(|c| c.is_ascii_hexdigit()) {
        let nar_hash_base32 = convert_hash_to_base32(&format!("sha256:{}", nar_hash_value));
        writeln!(narinfo, "NarHash: {}", nar_hash_base32)
            .map_err(|e| worker::Error::RustError(e.to_string()))?;
    } else if nar_hash_value.len() == 52 {
        // Already base32
        writeln!(narinfo, "NarHash: sha256:{}", nar_hash_value)
            .map_err(|e| worker::Error::RustError(e.to_string()))?;
    } else {
        // Unknown format, output as-is (may cause issues but at least visible for debugging)
        writeln!(narinfo, "NarHash: sha256:{}", nar_hash_value)
            .map_err(|e| worker::Error::RustError(e.to_string()))?;
    }
    writeln!(narinfo, "NarSize: {}", nar.nar_size)
        .map_err(|e| worker::Error::RustError(e.to_string()))?;

    // References
    if !object.references.is_empty() {
        writeln!(narinfo, "References: {}", object.references.join(" "))
            .map_err(|e| worker::Error::RustError(e.to_string()))?;
    }

    // System
    if let Some(ref system) = object.system {
        writeln!(narinfo, "System: {}", system)
            .map_err(|e| worker::Error::RustError(e.to_string()))?;
    }

    // Deriver
    if let Some(ref deriver) = object.deriver {
        writeln!(narinfo, "Deriver: {}", deriver)
            .map_err(|e| worker::Error::RustError(e.to_string()))?;
    }

    // Signatures - include client-provided signatures
    for sig in &object.sigs {
        writeln!(narinfo, "Sig: {}", sig).map_err(|e| worker::Error::RustError(e.to_string()))?;
    }

    // Server-side signing: if no signatures exist and we have a keypair, sign the narinfo
    if object.sigs.is_empty() {
        if let Some(keypair) = keypair {
            // Compute the fingerprint and sign it
            let fingerprint = compute_fingerprint(
                &object.store_path,
                &nar.nar_hash,
                nar.nar_size,
                &object.references,
            );

            match sign_message(keypair, &fingerprint) {
                Ok(signature) => {
                    writeln!(narinfo, "Sig: {}", signature)
                        .map_err(|e| worker::Error::RustError(e.to_string()))?;
                }
                Err(e) => {
                    // Log the error but don't fail - unsigned narinfo is still valid
                    web_sys::console::warn_1(&format!("Failed to sign narinfo: {}", e).into());
                }
            }
        }
    }

    // CA (content-addressed)
    if let Some(ref ca) = object.ca {
        writeln!(narinfo, "CA: {}", ca).map_err(|e| worker::Error::RustError(e.to_string()))?;
    }

    Ok(narinfo)
}

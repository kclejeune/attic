//! Binary Cache API handlers (Nix protocol).

use worker::*;

use crate::crypto::{compute_fingerprint, sign_message};
use crate::error::WorkerError;
use crate::state::WorkerState;

/// GET /:cache/nix-cache-info
///
/// Returns basic cache information for Nix.
pub async fn get_nix_cache_info(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
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

    // Build nix-cache-info response
    let info = format!(
        "StoreDir: {}\nWantMassQuery: 1\nPriority: {}\n",
        cache.store_dir, cache.priority
    );

    Response::ok(info)
}

/// GET /:cache/:path
///
/// Returns narinfo for a store path.
/// Path format: <hash>.narinfo
pub async fn get_store_path_info(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
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

/// GET /:cache/nar/:path
///
/// Returns the NAR file. For single-chunk NARs, redirects to R2.
/// For multi-chunk NARs, streams the concatenated chunks.
pub async fn get_nar(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let _cache_name = ctx.param("cache").unwrap_or(&String::new()).clone();
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
        // Single chunk - redirect to R2
        let chunk = &chunks[0];
        let remote_file: serde_json::Value = serde_json::from_str(&chunk.remote_file)
            .map_err(|e| worker::Error::RustError(format!("Invalid remote file: {}", e)))?;

        // Extract key from remote file
        let key = remote_file
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| worker::Error::RustError("No key in remote file".to_string()))?;

        // Download and return the file
        match state.storage.download_file(key).await {
            Ok(crate::storage::Download::Bytes(bytes)) => {
                let mut headers = Headers::new();
                headers.set("Content-Type", "application/x-nix-nar")?;
                if let Some(file_size) = chunk.file_size {
                    headers.set("Content-Length", &file_size.to_string())?;
                }
                Ok(Response::from_bytes(bytes.to_vec())?.with_headers(headers))
            }
            Ok(crate::storage::Download::Url(url)) => Response::redirect_with_status(
                Url::parse(&url)
                    .map_err(|e| worker::Error::RustError(format!("Invalid URL: {}", e)))?,
                302,
            ),
            Err(e) => Ok(e.to_response()),
        }
    } else {
        // Multi-chunk - stream concatenated chunks
        // TODO: Implement streaming for multi-chunk NARs
        Response::error("Multi-chunk NARs not yet supported", 501)
    }
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
    // Strip sha256: prefix if present for clean URL
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
            writeln!(narinfo, "FileHash: sha256:{}", file_hash)
                .map_err(|e| worker::Error::RustError(e.to_string()))?;
        }
        if let Some(file_size) = chunk.file_size {
            writeln!(narinfo, "FileSize: {}", file_size)
                .map_err(|e| worker::Error::RustError(e.to_string()))?;
        }
    }

    // NAR hash and size (uncompressed)
    // Strip prefix if already present (client may send "sha256:..." or just hash)
    let nar_hash = nar
        .nar_hash
        .strip_prefix("sha256:")
        .unwrap_or(&nar.nar_hash);
    writeln!(narinfo, "NarHash: sha256:{}", nar_hash)
        .map_err(|e| worker::Error::RustError(e.to_string()))?;
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

//! Cache configuration handlers.

use serde::Deserialize;
use worker::*;

use crate::crypto;
use crate::error::WorkerError;
use crate::state::{RequestState, WorkerState};

/// Keypair configuration options.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum KeypairConfig {
    /// Generate a new keypair.
    Generate,
    /// Set a specific keypair (for importing).
    Set { keypair: String },
}

/// Request body for cache configuration update.
#[derive(Debug, Deserialize)]
struct CacheConfigUpdate {
    is_public: Option<bool>,
    priority: Option<i32>,
    compression: Option<String>,
    retention_period: Option<Option<i32>>,
    upstream_cache_key_names: Option<Vec<String>>,
    keypair: Option<KeypairConfig>,
}

/// GET /:cache/attic-cache-info or GET /_api/v1/cache-config/:cache
///
/// Returns cache configuration.
pub async fn get_cache_config(req: Request, ctx: RouteContext<()>) -> Result<Response> {
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

    // Build base URL from request (including port if present)
    let url = req.url()?;
    let host = url.host_str().unwrap_or("localhost");
    let base_url = match url.port() {
        Some(port) => format!("{}://{}:{}", url.scheme(), host, port),
        None => format!("{}://{}", url.scheme(), host),
    };

    // Extract public key from keypair
    let public_key =
        crypto::extract_public_key(&cache.keypair).unwrap_or_else(|_| cache.keypair.clone());

    // Build response with absolute URLs and worker capabilities
    let config = serde_json::json!({
        "substituter_endpoint": format!("{}/{}/", base_url, cache_name),
        "api_endpoint": format!("{}/", base_url),
        "public_key": public_key,
        "is_public": cache.is_public,
        "store_dir": cache.store_dir,
        "priority": cache.priority,
        "compression": cache.compression,
        // Worker capabilities for client feature detection
        "worker_capabilities": {
            // Supports preamble-based NAR info for large metadata
            "preamble_nar_info": true,
            // Server-side signing is enabled
            "server_signing": true,
            // Server-side compression is enabled (client sends uncompressed)
            "server_compression": true,
        },
    });

    Response::from_json(&config)
}

/// POST /_api/v1/cache-config/:cache
///
/// Creates a new cache.
pub async fn create_cache(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let cache_name = ctx.param("cache").unwrap_or(&String::new()).clone();

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

    // Check permission to create cache
    let cache_name_parsed = attic::cache::CacheName::new(cache_name.clone())
        .map_err(|e| WorkerError::BadRequest(format!("Invalid cache name: {}", e)))?;
    let permission = token.get_permission_for_cache(&cache_name_parsed);
    if let Err(e) = permission.require_create_cache() {
        return Ok(WorkerError::Authorization(format!("Permission denied: {:?}", e)).to_response());
    }

    // Parse request body
    let body: serde_json::Value = req
        .json()
        .await
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    // Generate Ed25519 keypair for signing
    let keypair = match crypto::generate_keypair(&cache_name) {
        Ok(kp) => kp,
        Err(e) => return Ok(e.to_response()),
    };

    // Extract public key before creating cache (keypair will be moved into struct)
    let public_key = crypto::extract_public_key(&keypair).ok();

    // Get compression from request or use default
    let compression = body
        .get("compression")
        .and_then(|v| v.as_str())
        .map(validate_compression)
        .transpose()
        .map_err(|e| worker::Error::RustError(e))?
        .unwrap_or_else(|| "br".to_string());

    // Create cache
    let cache = crate::database::Cache {
        id: None,
        name: cache_name.clone(),
        keypair,
        is_public: body
            .get("is_public")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        store_dir: body
            .get("store_dir")
            .and_then(|v| v.as_str())
            .unwrap_or("/nix/store")
            .to_string(),
        priority: body.get("priority").and_then(|v| v.as_i64()).unwrap_or(40) as i32,
        upstream_cache_key_names: Vec::new(),
        compression,
        created_at: chrono::Utc::now().to_rfc3339(),
        deleted_at: None,
        retention_period: body
            .get("retention_period")
            .and_then(|v| v.as_i64())
            .map(|n| n as i32),
    };

    match state.database.create_cache(&cache).await {
        Ok(_) => {
            let response = serde_json::json!({
                "name": cache_name,
                "created": true,
                "public_key": public_key,
            });
            Response::from_json(&response)
        }
        Err(e) => Ok(e.to_response()),
    }
}

/// PATCH /_api/v1/cache-config/:cache
///
/// Updates cache configuration.
pub async fn configure_cache(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let cache_name = ctx.param("cache").unwrap_or(&String::new()).clone();

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

    // Check permission to configure cache (allow if user can configure or create cache)
    let cache_name_parsed = attic::cache::CacheName::new(cache_name.clone())
        .map_err(|e| WorkerError::BadRequest(format!("Invalid cache name: {}", e)))?;
    let permission = token.get_permission_for_cache(&cache_name_parsed);
    // Allow configure if user has configure_cache OR create_cache permission
    if permission.require_configure_cache().is_err() && permission.require_create_cache().is_err() {
        return Ok(WorkerError::Authorization(
            "Permission denied: requires configure or create cache permission".to_string(),
        )
        .to_response());
    }

    // Parse request body
    let body: CacheConfigUpdate = req
        .json()
        .await
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    // Validate compression if provided
    let compression = body
        .compression
        .as_ref()
        .map(|c| validate_compression(c))
        .transpose()
        .map_err(|e| worker::Error::RustError(e))?;

    // Check if cache exists
    match state.database.find_cache(&cache_name).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Ok(
                WorkerError::NotFound(format!("Cache not found: {}", cache_name)).to_response(),
            )
        }
        Err(e) => return Ok(e.to_response()),
    }

    // Handle keypair configuration
    let keypair = match body.keypair {
        Some(KeypairConfig::Generate) => {
            // Generate new Ed25519 keypair
            match crypto::generate_keypair(&cache_name) {
                Ok(kp) => Some(kp),
                Err(e) => return Ok(e.to_response()),
            }
        }
        Some(KeypairConfig::Set { keypair }) => {
            // Validate the keypair format
            if let Err(e) = crypto::extract_public_key(&keypair) {
                return Ok(WorkerError::BadRequest(format!("Invalid keypair: {}", e)).to_response());
            }
            Some(keypair)
        }
        None => None,
    };

    // Update cache configuration
    match state
        .database
        .update_cache(
            &cache_name,
            body.is_public,
            body.priority,
            compression.as_deref(),
            body.retention_period,
            body.upstream_cache_key_names.as_deref(),
            keypair.as_deref(),
        )
        .await
    {
        Ok(_) => {
            // If keypair was regenerated, return the new public key
            let mut response = serde_json::json!({
                "name": cache_name,
                "updated": true,
            });

            if let Some(kp) = &keypair {
                if let Ok(public_key) = crypto::extract_public_key(kp) {
                    response["public_key"] = serde_json::json!(public_key);
                }
            }

            Response::from_json(&response)
        }
        Err(e) => Ok(e.to_response()),
    }
}

/// DELETE /_api/v1/cache-config/:cache
///
/// Soft-deletes a cache by setting its deleted_at timestamp.
/// The cache will no longer be accessible but data is preserved for potential recovery.
pub async fn destroy_cache(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let cache_name = ctx.param("cache").unwrap_or(&String::new()).clone();

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

    // Check permission to destroy cache
    let cache_name_parsed = attic::cache::CacheName::new(cache_name.clone())
        .map_err(|e| WorkerError::BadRequest(format!("Invalid cache name: {}", e)))?;
    let permission = token.get_permission_for_cache(&cache_name_parsed);
    if let Err(e) = permission.require_destroy_cache() {
        return Ok(WorkerError::Authorization(format!("Permission denied: {:?}", e)).to_response());
    }

    // Delete the cache
    match state.database.delete_cache(&cache_name).await {
        Ok(true) => {
            let response = serde_json::json!({
                "name": cache_name,
                "deleted": true,
            });
            Response::from_json(&response)
        }
        Ok(false) => {
            Ok(WorkerError::NotFound(format!("Cache not found: {}", cache_name)).to_response())
        }
        Err(e) => Ok(e.to_response()),
    }
}

/// Validate compression type string.
fn validate_compression(compression: &str) -> std::result::Result<String, String> {
    match compression {
        "none" | "zstd" | "br" | "brotli" | "gzip" | "gz" | "xz" | "lzma" => {
            Ok(match compression {
                "brotli" => "br".to_string(),
                "gz" => "gzip".to_string(),
                "lzma" => "xz".to_string(),
                _ => compression.to_string(),
            })
        }
        _ => Err(format!(
            "Invalid compression type: {}. Valid options: none, zstd, br (brotli), gzip, xz",
            compression
        )),
    }
}

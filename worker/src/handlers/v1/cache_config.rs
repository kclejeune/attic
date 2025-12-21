//! Cache configuration handlers.

use worker::*;

use crate::error::WorkerError;
use crate::state::{RequestState, WorkerState};

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

    // Build response with absolute URLs
    let config = serde_json::json!({
        "substituter_endpoint": format!("{}/{}/", base_url, cache_name),
        "api_endpoint": format!("{}/", base_url),
        "public_key": cache.keypair, // TODO: Extract public key from keypair
        "is_public": cache.is_public,
        "store_dir": cache.store_dir,
        "priority": cache.priority,
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
        None => return Ok(WorkerError::Authentication("No token provided".to_string()).to_response()),
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

    // Generate keypair for signing
    // TODO: Generate Ed25519 keypair
    let keypair = format!("{}:placeholder-keypair", cache_name);

    // Create cache
    let cache = crate::database::Cache {
        id: None,
        name: cache_name.clone(),
        keypair,
        is_public: body.get("is_public").and_then(|v| v.as_bool()).unwrap_or(false),
        store_dir: body
            .get("store_dir")
            .and_then(|v| v.as_str())
            .unwrap_or("/nix/store")
            .to_string(),
        priority: body.get("priority").and_then(|v| v.as_i64()).unwrap_or(40) as i32,
        upstream_cache_key_names: Vec::new(),
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
            });
            Response::from_json(&response)
        }
        Err(e) => Ok(e.to_response()),
    }
}

/// PATCH /_api/v1/cache-config/:cache
///
/// Updates cache configuration.
pub async fn configure_cache(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let cache_name = ctx.param("cache").unwrap_or(&String::new()).clone();

    // TODO: Implement cache configuration update
    Response::error(format!("Cache configuration update not yet implemented for {}", cache_name), 501)
}

/// DELETE /_api/v1/cache-config/:cache
///
/// Deletes a cache.
pub async fn destroy_cache(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let cache_name = ctx.param("cache").unwrap_or(&String::new()).clone();

    // TODO: Implement cache deletion
    Response::error(format!("Cache deletion not yet implemented for {}", cache_name), 501)
}

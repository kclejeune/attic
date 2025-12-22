//! Get missing paths handler.

use serde::{Deserialize, Serialize};
use worker::*;

use crate::error::WorkerError;
use crate::state::{RequestState, WorkerState};

/// Request body for get-missing-paths.
#[derive(Deserialize)]
struct GetMissingPathsRequest {
    cache: String,
    store_path_hashes: Vec<String>,
}

/// Response for get-missing-paths.
#[derive(Serialize)]
struct GetMissingPathsResponse {
    missing_paths: Vec<String>,
}

/// POST /_api/v1/get-missing-paths
///
/// Returns which store path hashes are missing from the cache.
pub async fn get_missing_paths(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    let req_state = match RequestState::from_request(&req, &state.jwt_config) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    // Parse request body
    let body: GetMissingPathsRequest = req
        .json()
        .await
        .map_err(|e| worker::Error::RustError(format!("Invalid JSON: {}", e)))?;

    // Check authentication
    let token = match req_state.token {
        Some(t) => t,
        None => {
            return Ok(WorkerError::Authentication("No token provided".to_string()).to_response())
        }
    };

    // Check permission to push (need push permission to query missing paths)
    let cache_name = attic::cache::CacheName::new(body.cache.clone())
        .map_err(|e| WorkerError::BadRequest(format!("Invalid cache name: {}", e)))?;
    let permission = token.get_permission_for_cache(&cache_name);
    if let Err(e) = permission.require_push() {
        return Ok(WorkerError::Authorization(format!("Permission denied: {:?}", e)).to_response());
    }

    // Find existing paths
    let existing = match state
        .database
        .find_existing_paths(&body.cache, &body.store_path_hashes)
        .await
    {
        Ok(e) => e,
        Err(e) => return Ok(e.to_response()),
    };

    // Compute missing paths
    let existing_set: std::collections::HashSet<_> = existing.into_iter().collect();
    let missing_paths: Vec<String> = body
        .store_path_hashes
        .into_iter()
        .filter(|h| !existing_set.contains(h))
        .collect();

    let response = GetMissingPathsResponse { missing_paths };
    Response::from_json(&response)
}

//! Manual garbage-collection trigger for the admin UI.

use worker::*;

use crate::error::WorkerError;
use crate::state::{RequestState, WorkerState};

/// POST /_api/v1/gc
///
/// Runs a garbage-collection pass on demand and returns the reclaimed counts.
/// Requires a token with broad delete authority (the admin app mints a wildcard
/// token for this); the same sweeps also run on the scheduled cron.
pub async fn run_gc(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    let req_state = match RequestState::from_request(&req, &state).await {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    let token = match req_state.token {
        Some(t) => t,
        None => {
            return Ok(WorkerError::Authentication("No token provided".to_string()).to_response())
        }
    };

    // GC is a global operation; require delete authority across caches. A
    // wildcard-scoped admin token satisfies this; a single-cache token does not.
    let probe = attic::cache::CacheName::new("gc".to_string())
        .map_err(|e| WorkerError::Internal(format!("Invalid probe name: {}", e)))?;
    if token
        .get_permission_for_cache(&probe)
        .require_delete()
        .is_err()
    {
        return Ok(
            WorkerError::Authorization("Delete permission required to run GC".to_string())
                .to_response(),
        );
    }

    let stats = crate::gc::run(&state).await;
    Response::from_json(&stats)
}

//! OAuth device-authorization endpoints for headless CLI login.
//!
//! These are machine-to-machine and unauthenticated: the user authenticates in
//! a browser on the admin app, which approves the grant. The CLI polls here.

use serde::Deserialize;
use worker::*;

use crate::error::{WorkerError, WorkerResult};
use crate::state::WorkerState;

const DEVICE_CODE_EXPIRY_SECS: i64 = 600;
const POLL_INTERVAL_SECS: u32 = 5;
/// Unambiguous user-code alphabet (no 0/O/1/I).
const USER_CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn random_hex(bytes: usize) -> WorkerResult<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).map_err(|e| WorkerError::Internal(format!("RNG: {}", e)))?;
    Ok(buf.iter().map(|b| format!("{:02x}", b)).collect())
}

fn user_code() -> WorkerResult<String> {
    let mut buf = [0u8; 8];
    getrandom::getrandom(&mut buf).map_err(|e| WorkerError::Internal(format!("RNG: {}", e)))?;
    let code: String = buf
        .iter()
        .map(|b| USER_CODE_ALPHABET[(*b as usize) % USER_CODE_ALPHABET.len()] as char)
        .collect();
    Ok(format!("{}-{}", &code[..4], &code[4..]))
}

/// POST /_api/v1/cli/device — begin a device-authorization grant.
pub async fn device_start(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    let device_code = match random_hex(32) {
        Ok(c) => c,
        Err(e) => return Ok(e.to_response()),
    };
    let user_code = match user_code() {
        Ok(c) => c,
        Err(e) => return Ok(e.to_response()),
    };
    let expires_at = chrono::Utc::now().timestamp() + DEVICE_CODE_EXPIRY_SECS;

    if let Err(e) = state
        .database
        .create_device_auth(&device_code, &user_code, expires_at)
        .await
    {
        return Ok(e.to_response());
    }

    let admin = ctx
        .env
        .var("CLI_AUTH_BASE_URL")
        .map(|v| v.to_string())
        .unwrap_or_default();
    let admin = admin.trim_end_matches('/');

    Response::from_json(&serde_json::json!({
        "device_code": device_code,
        "user_code": user_code,
        "verification_uri": format!("{}/cli/device", admin),
        "verification_uri_complete": format!("{}/cli/device?code={}", admin, user_code),
        "interval": POLL_INTERVAL_SECS,
        "expires_in": DEVICE_CODE_EXPIRY_SECS,
    }))
}

#[derive(Deserialize)]
struct TokenRequest {
    device_code: String,
}

/// POST /_api/v1/cli/token — poll for the approved token.
pub async fn device_token(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let state = match WorkerState::from_env(&ctx.env) {
        Ok(s) => s,
        Err(e) => return Ok(e.to_response()),
    };

    let body: TokenRequest = match req.json().await {
        Ok(b) => b,
        Err(e) => return Ok(WorkerError::BadRequest(format!("Invalid body: {}", e)).to_response()),
    };

    let grant = match state.database.find_device_auth(&body.device_code).await {
        Ok(Some(g)) => g,
        Ok(None) => return device_error("expired_token"),
        Err(e) => return Ok(e.to_response()),
    };

    if grant.expires_at < chrono::Utc::now().timestamp() {
        let _ = state.database.delete_device_auth(&grant.device_code).await;
        return device_error("expired_token");
    }

    match grant.status.as_str() {
        "approved" => {
            let token = grant.token.clone().unwrap_or_default();
            // One-time retrieval.
            let _ = state.database.delete_device_auth(&grant.device_code).await;
            Response::from_json(&serde_json::json!({ "token": token }))
        }
        "denied" => {
            let _ = state.database.delete_device_auth(&grant.device_code).await;
            device_error("access_denied")
        }
        _ => device_error("authorization_pending"),
    }
}

/// Device-flow error responses use HTTP 400 with an `error` code (RFC 8628).
fn device_error(code: &str) -> Result<Response> {
    Ok(Response::from_json(&serde_json::json!({ "error": code }))?.with_status(400))
}

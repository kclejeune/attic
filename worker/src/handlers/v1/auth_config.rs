//! CLI authentication discovery.
//!
//! Lets the `attic` client learn where to send the user for interactive login
//! without hardcoding the admin URL.

use worker::*;

/// GET /_api/v1/auth-config
///
/// Public discovery document for the CLI login flows. Device endpoints are
/// relative to this API; the browser-facing URLs live on the admin app.
pub async fn auth_config(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let admin = ctx
        .env
        .var("CLI_AUTH_BASE_URL")
        .map(|v| v.to_string())
        .unwrap_or_default();
    let admin = admin.trim_end_matches('/');

    let config = serde_json::json!({
        "authorize_url": if admin.is_empty() { serde_json::Value::Null } else { format!("{}/cli", admin).into() },
        "device_verification_url": if admin.is_empty() { serde_json::Value::Null } else { format!("{}/cli/device", admin).into() },
        "device_authorization_endpoint": "/_api/v1/cli/device",
        "token_endpoint": "/_api/v1/cli/token",
    });

    Response::from_json(&config)
}

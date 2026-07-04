//! Attic Cloudflare Worker
//!
//! A serverless Nix binary cache server running on Cloudflare Workers
//! with R2 storage and D1/Turso database support.

#![deny(
    asm_sub_register,
    deprecated,
    missing_abi,
    unsafe_code,
    unused_macros,
    unused_must_use,
    unused_unsafe
)]

mod compression;
mod crypto;
mod database;
mod error;
mod gc;
mod handlers;
mod state;
mod storage;
mod streaming;

use worker::*;

use crate::handlers::{binary_cache, v1};
use crate::state::WorkerState;

/// Main entry point for the Cloudflare Worker.
#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    // Set up panic hook for better error messages in development
    console_error_panic_hook::set_once();

    // Initialize zstd-wasm (idempotent, only runs once)
    if let Err(e) = compression::js_zstd::init().await {
        console_log!("Warning: Failed to initialize zstd-wasm: {}", e);
    }

    // Initialize router
    let router = Router::new();

    router
        // Health check
        .get_async("/", |_, _| async move {
            Response::ok("Attic Worker is running")
        })
        // Binary Cache API (Nix protocol)
        // GET endpoints
        .get_async("/:cache/nix-cache-info", binary_cache::get_nix_cache_info)
        .get_async("/:cache/:path", binary_cache::get_store_path_info)
        .get_async("/:cache/nar/:path", binary_cache::get_nar)
        // HEAD endpoints (Nix uses HEAD to check path existence)
        .head_async("/:cache/nix-cache-info", binary_cache::head_nix_cache_info)
        .head_async("/:cache/:path", binary_cache::head_store_path_info)
        .head_async("/:cache/nar/:path", binary_cache::head_nar)
        // Attic API v1
        .get_async(
            "/:cache/attic-cache-info",
            v1::cache_config::get_cache_config,
        )
        .get_async(
            "/_api/v1/cache-config/:cache",
            v1::cache_config::get_cache_config,
        )
        .post_async(
            "/_api/v1/cache-config/:cache",
            v1::cache_config::create_cache,
        )
        .patch_async(
            "/_api/v1/cache-config/:cache",
            v1::cache_config::configure_cache,
        )
        .delete_async(
            "/_api/v1/cache-config/:cache",
            v1::cache_config::destroy_cache,
        )
        .post_async(
            "/_api/v1/cache-config/:cache/rename",
            v1::cache_config::rename_cache,
        )
        .post_async(
            "/_api/v1/get-missing-paths",
            v1::get_missing_paths::get_missing_paths,
        )
        .put_async("/_api/v1/upload-path", v1::upload_path::upload_path)
        // Chunked upload endpoints for large files (>100MB)
        .post_async(
            "/_api/v1/upload-path/start",
            v1::upload_path::start_chunked_upload,
        )
        .put_async("/_api/v1/upload-path/chunk", v1::upload_path::upload_chunk)
        .post_async(
            "/_api/v1/upload-path/complete",
            v1::upload_path::complete_chunked_upload,
        )
        // Admin-triggered garbage collection
        .post_async("/_api/v1/gc", v1::gc::run_gc)
        // CLI login discovery + device-authorization flow
        .get_async("/_api/v1/auth-config", v1::auth_config::auth_config)
        .post_async("/_api/v1/cli/device", v1::cli::device_start)
        .post_async("/_api/v1/cli/token", v1::cli::device_token)
        .run(req, env)
        .await
}

/// Scheduled (cron) entry point for garbage collection.
#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    console_error_panic_hook::set_once();

    let state = match WorkerState::from_env(&env) {
        Ok(s) => s,
        Err(e) => {
            console_log!("gc: failed to build worker state: {}", e);
            return;
        }
    };

    let stats = gc::run(&state).await;
    console_log!(
        "gc: reaped {} abandoned uploads ({} errors)",
        stats.abandoned_uploads_reaped,
        stats.abandoned_upload_errors
    );
}

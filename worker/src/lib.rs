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
mod database;
mod error;
mod handlers;
mod state;
mod storage;
mod streaming;

use worker::*;

use crate::handlers::{binary_cache, v1};

/// Main entry point for the Cloudflare Worker.
#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    // Set up panic hook for better error messages in development
    console_error_panic_hook::set_once();

    // Initialize router
    let router = Router::new();

    router
        // Health check
        .get_async("/", |_, _| async move {
            Response::ok("Attic Worker is running")
        })
        // Binary Cache API (Nix protocol)
        .get_async("/:cache/nix-cache-info", binary_cache::get_nix_cache_info)
        .get_async("/:cache/:path", binary_cache::get_store_path_info)
        // NAR downloads - match .nar and .nar.* extensions
        .get_async("/:cache/nar/:path", binary_cache::get_nar)
        // Attic API v1
        .get_async("/:cache/attic-cache-info", v1::cache_config::get_cache_config)
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
            "/_api/v1/get-missing-paths",
            v1::get_missing_paths::get_missing_paths,
        )
        .put_async("/_api/v1/upload-path", v1::upload_path::upload_path)
        .run(req, env)
        .await
}

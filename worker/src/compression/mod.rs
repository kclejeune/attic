//! Compression utilities for WASM.
//!
//! Provides zstd compression using ruzstd (pure Rust) for Cloudflare Workers.

mod config;
mod stream;

pub use config::CompressionConfig;
pub use stream::compress_buffer;

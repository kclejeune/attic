//! JavaScript zstd-wasm bindings via wasm-bindgen.
//!
//! This module provides zstd compression using @bokuweb/zstd-wasm through
//! JavaScript interop. The WASM module is initialized via the global __zstd object
//! which is set up by the JavaScript entry point.

use js_sys::{Function, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::error::{WorkerError, WorkerResult};

/// Get the global __zstd object.
fn get_zstd_global() -> WorkerResult<Object> {
    let global = js_sys::global();
    let zstd = Reflect::get(&global, &JsValue::from_str("__zstd"))
        .map_err(|_| WorkerError::Compression("__zstd global not found".to_string()))?;

    if zstd.is_undefined() || zstd.is_null() {
        return Err(WorkerError::Compression(
            "__zstd global not initialized - entry.js not loaded".to_string(),
        ));
    }

    zstd.dyn_into::<Object>()
        .map_err(|_| WorkerError::Compression("__zstd is not an object".to_string()))
}

/// Initialize the zstd WASM module.
/// This must be called once before any compression/decompression operations.
pub async fn init() -> WorkerResult<()> {
    let zstd = get_zstd_global()?;

    // Check if already initialized
    let initialized = Reflect::get(&zstd, &JsValue::from_str("initialized")).map_err(|e| {
        WorkerError::Compression(format!("Failed to get initialized flag: {:?}", e))
    })?;

    if initialized.as_bool().unwrap_or(false) {
        return Ok(());
    }

    // Call init()
    let init_fn = Reflect::get(&zstd, &JsValue::from_str("init"))
        .map_err(|e| WorkerError::Compression(format!("Failed to get init function: {:?}", e)))?;

    let init_fn: Function = init_fn
        .dyn_into()
        .map_err(|_| WorkerError::Compression("init is not a function".to_string()))?;

    let promise = init_fn
        .call0(&JsValue::undefined())
        .map_err(|e| WorkerError::Compression(format!("Failed to call init: {:?}", e)))?;

    JsFuture::from(js_sys::Promise::from(promise))
        .await
        .map_err(|e| WorkerError::Compression(format!("zstd init failed: {:?}", e)))?;

    Ok(())
}

/// Compress data using zstd.
///
/// # Arguments
/// * `data` - The data to compress
/// * `level` - Compression level (1-22, higher = better compression but slower)
///
/// # Returns
/// The compressed data as a Vec<u8>
pub fn compress(data: &[u8], level: u32) -> WorkerResult<Vec<u8>> {
    let zstd = get_zstd_global()?;

    // Check if initialized
    let initialized = Reflect::get(&zstd, &JsValue::from_str("initialized"))
        .map_err(|e| WorkerError::Compression(format!("Failed to check initialized: {:?}", e)))?;

    if !initialized.as_bool().unwrap_or(false) {
        return Err(WorkerError::Compression(
            "zstd not initialized - call init() first".to_string(),
        ));
    }

    // Get compress function
    let compress_fn = Reflect::get(&zstd, &JsValue::from_str("compress")).map_err(|e| {
        WorkerError::Compression(format!("Failed to get compress function: {:?}", e))
    })?;

    let compress_fn: Function = compress_fn
        .dyn_into()
        .map_err(|_| WorkerError::Compression("compress is not a function".to_string()))?;

    // Convert input to Uint8Array
    let input = Uint8Array::from(data);

    // Call compress(data, level)
    let result = compress_fn
        .call2(&JsValue::undefined(), &input, &JsValue::from(level))
        .map_err(|e| WorkerError::Compression(format!("zstd compress failed: {:?}", e)))?;

    // Convert result to Vec<u8>
    let result_array: Uint8Array = result
        .dyn_into()
        .map_err(|_| WorkerError::Compression("compress result is not a Uint8Array".to_string()))?;

    Ok(result_array.to_vec())
}

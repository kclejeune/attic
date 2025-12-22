// Custom entry point for the Attic Worker
// Handles zstd-wasm initialization with manual WASM loading for Cloudflare Workers

// IMPORTANT: Import order matters!
// 1. First: Set up Module.instantiateWasm before any Emscripten code runs
import "./zstd-setup.js";

// 2. Then: Import the zstd module (will use our custom WASM loader)
// Using local copies to avoid package export restrictions
import { Module, waitInitialized } from "./zstd-lib/module.js";
import { compress } from "./zstd-lib/simple/compress.js";
import { decompress } from "./zstd-lib/simple/decompress.js";

// 3. Finally: Import the Rust worker
import rustWorker from "../build/worker/shim.mjs";

// Track initialization
let zstdInitialized = false;

async function initZstd() {
    if (zstdInitialized) return;

    try {
        // Call Module.init() to trigger Emscripten initialization
        // Pass empty string - our instantiateWasm override handles WASM loading
        Module.init("");

        // Wait for Emscripten to finish initialization
        await waitInitialized();

        zstdInitialized = true;
        globalThis.__zstd.initialized = true;
        globalThis.__zstd.compress = compress;
        globalThis.__zstd.decompress = decompress;

        console.log("zstd-wasm initialized successfully");
    } catch (e) {
        console.error("Failed to initialize zstd-wasm:", e);
        throw e;
    }
}

// Expose zstd functions to global scope for Rust access
globalThis.__zstd = {
    initialized: false,
    compress: null,
    decompress: null,
    init: initZstd
};

// Export the worker handler
export default rustWorker;

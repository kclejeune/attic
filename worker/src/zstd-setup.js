// Setup WASM loader for Emscripten BEFORE any zstd imports
// This file must be imported first to set up the Module global

import zstdWasmModule from "./zstd.wasm";

// Set up Module object that Emscripten's zstd.js will pick up
globalThis.Module = {
    instantiateWasm: (imports, successCallback) => {
        // Cloudflare Workers: instantiate from imported WASM module
        WebAssembly.instantiate(zstdWasmModule, imports)
            .then(instance => {
                successCallback(instance);
            })
            .catch(err => {
                console.error("Failed to instantiate zstd WASM:", err);
            });
        return {}; // Return empty, actual exports come via callback
    }
};

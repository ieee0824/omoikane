# FFI Samples

This directory contains small compatibility samples for the generated `cdylib`.

- `python_ctypes.py`: Python `ctypes` smoke call for `init`, `navigate`, and `evaluate`
- `node_ffi_napi.js`: Node.js sample using `ffi-napi`
- `puppeteer_bridge.js`: Puppeteer-side sketch that talks to the FFI layer
- `playwright_bridge.js`: Playwright-side sketch that talks to the FFI layer

The Rust test suite validates the exported C ABI directly. These samples are included as manual compatibility fixtures and are not executed automatically in this repository.

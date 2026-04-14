//! TBP adapter around the blockfish engine.
//!
//! Exposes the blockfish search over the message-based TBP protocol.
//! On wasm32, a `#[wasm_bindgen] pub fn start()`
//! entry point hooks into a Web Worker's `self.onmessage` / `postMessage` loop
//! and runs the search synchronously, no threading, no sub-workers.

pub mod adapter;
pub mod bot;
pub mod coords;
pub mod dispatch;

#[cfg(target_arch = "wasm32")]
mod wasm;

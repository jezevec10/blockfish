//! WASM entry point for the blockfish TBP bot.
//!
//! Bootstraps a `DedicatedWorkerGlobalScope` message loop that:
//!   1. Posts an `info` greeting at startup.
//!   2. Deserializes inbound `MessageEvent.data` into `tbp::FrontendMessage`.
//!   3. Routes through `dispatch::handle` against shared `Bot` state.
//!   4. Posts the optional reply back via `postMessage`.

use crate::bot::Bot;
use crate::dispatch::{handle, make_info};
use blockfish::{srs, ShapeTable};
use std::cell::RefCell;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(thread_local_v2, js_name = self)]
    static SELF: web_sys::DedicatedWorkerGlobalScope;
}

thread_local! {
    /// Active bot session. `None` until the host sends `start`; reset to
    /// `None` on `stop` or fatal queue desync.
    static BOT: RefCell<Option<Bot>> = const { RefCell::new(None) };
    /// SRS shape table, lazily constructed on first access.
    static SHTB: ShapeTable = srs();
}

/// Bootstrap entry called from the worker's `blockfish_main.js` loader.
#[wasm_bindgen]
pub fn start() {
    #[cfg(feature = "panic-hook")]
    console_error_panic_hook::set_once();

    // Wire up self.onmessage - on_message - dispatch
    let closure = Closure::wrap(Box::new(|e: web_sys::MessageEvent| {
        on_message(e.data());
    }) as Box<dyn FnMut(web_sys::MessageEvent)>);
    SELF.with(|s| {
        s.add_event_listener_with_callback("message", closure.as_ref().unchecked_ref())
            .expect("addEventListener should not fail in a Web Worker scope");
    });
    // The closure must outlive `start()` for the lifetime of the worker.
    closure.forget();

    // Emit the mandatory `info` greeting.
    post(&make_info());
}

/// Handle one inbound message from the host.
#[allow(clippy::explicit_auto_deref)]
fn on_message(data: JsValue) {
    let msg: tbp::FrontendMessage = match serde_wasm_bindgen::from_value(data) {
        Ok(m) => m,
        Err(e) => {
            warn(&format!("blockfish-tbp: bad message: {}", e));
            return;
        }
    };
    let reply = SHTB.with(|shtb| BOT.with(|cell| handle(&mut *cell.borrow_mut(), shtb, msg)));
    if let Some(reply) = reply {
        post(&reply);
    }
}

/// Serialize a `BotMessage` and post it to the host.
fn post(msg: &tbp::BotMessage) {
    match serde_wasm_bindgen::to_value(msg) {
        Ok(js) => {
            SELF.with(|s| {
                if let Err(e) = s.post_message(&js) {
                    warn(&format!("blockfish-tbp: post_message failed: {:?}", e));
                }
            });
        }
        Err(e) => warn(&format!("blockfish-tbp: serialize failed: {}", e)),
    }
}

fn warn(msg: &str) {
    web_sys::console::warn_1(&JsValue::from_str(msg));
}

//! Unexpected errors in the browser reach GlitchTip through the api
//! (`POST /api/client-errors`). A panic is reported with a plain `fetch` (the
//! app may be unusable by then), as well as logged to the console.

use js_sys::{Object, Reflect};
use pedagog_core::api::ClientError;
use wasm_bindgen::JsValue;

const VERSION: &str = concat!("pedagog-web@", env!("CARGO_PKG_VERSION"));

pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        console_error_panic_hook::hook(info);
        send(&info.to_string());
    }));
}

/// Report `message` from the current page (fire and forget).
pub fn send(message: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let path = window.location().pathname().unwrap_or_default();
    let report = ClientError {
        path,
        message: message.chars().take(4000).collect(),
        version: VERSION.into(),
    };
    let Ok(body) = serde_json::to_string(&report) else {
        return;
    };
    let headers = Object::new();
    let init = Object::new();
    let set = |o: &Object, k: &str, v: JsValue| {
        let _ = Reflect::set(o, &JsValue::from_str(k), &v);
    };
    set(&headers, "content-type", "application/json".into());
    set(&headers, "x-pedagog", "web".into());
    set(&init, "method", "POST".into());
    set(&init, "headers", headers.into());
    set(&init, "body", body.into());
    set(&init, "keepalive", true.into());
    if let Ok(fetch) = Reflect::get(&window, &JsValue::from_str("fetch")) {
        let fetch: js_sys::Function = fetch.into();
        let _ = fetch.call2(&window, &JsValue::from_str("/api/client-errors"), &init);
    }
}

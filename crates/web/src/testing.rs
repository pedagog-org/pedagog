//! Component tests in a headless browser (`cargo xtask web-test`): mount a
//! view into its own element and look at the DOM it made.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// A fresh element in the page with `view` mounted in it (left mounted).
pub fn mount<V: IntoView + 'static>(view: impl FnOnce() -> V + 'static) -> web_sys::HtmlElement {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .expect("a document");
    let root: web_sys::HtmlElement = document
        .create_element("div")
        .expect("an element")
        .unchecked_into();
    document
        .body()
        .expect("a body")
        .append_child(&root)
        .expect("appended");
    std::mem::forget(leptos::mount::mount_to(root.clone(), view));
    root
}

/// Every element under `root` matching `selector`.
pub fn all(root: &web_sys::HtmlElement, selector: &str) -> Vec<web_sys::Element> {
    let list = root.query_selector_all(selector).expect("a selector");
    (0..list.length())
        .filter_map(|i| list.item(i)?.dyn_into().ok())
        .collect()
}

/// The text of the first element matching `selector`.
pub fn text(root: &web_sys::HtmlElement, selector: &str) -> String {
    root.query_selector(selector)
        .ok()
        .flatten()
        .and_then(|e| e.text_content())
        .unwrap_or_default()
}

/// Let pending tasks and renders run.
pub async fn settle(ms: u64) {
    let (done, wait) = futures::channel::oneshot::channel::<()>();
    set_timeout(
        move || {
            let _ = done.send(());
        },
        std::time::Duration::from_millis(ms),
    );
    let _ = wait.await;
}

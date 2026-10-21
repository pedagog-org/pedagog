//! A dropdown: a button that opens a menu below it, closed by Escape, a click
//! elsewhere, or choosing an item.

use leptos::ev;
use leptos::prelude::*;

/// Open/closed state for one dropdown, closed by Escape or an outside click.
#[derive(Clone, Copy)]
pub struct Dropdown {
    pub open: RwSignal<bool>,
    pub root: NodeRef<leptos::html::Div>,
}

pub fn dropdown() -> Dropdown {
    let d = Dropdown {
        open: RwSignal::new(false),
        root: NodeRef::new(),
    };
    let on_key = window_event_listener(ev::keydown, move |e| {
        if e.key() == "Escape" {
            d.open.set(false);
        }
    });
    let on_click = window_event_listener(ev::click, move |e| {
        let inside = d
            .root
            .get_untracked()
            .zip(e.target())
            .is_some_and(|(root, target)| {
                wasm_bindgen::JsCast::dyn_ref::<web_sys::Node>(&target)
                    .is_some_and(|n| root.contains(Some(n)))
            });
        if !inside {
            d.open.set(false);
        }
    });
    on_cleanup(move || {
        on_key.remove();
        on_click.remove();
    });
    d
}

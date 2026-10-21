//! Light or dark: the system's by default, or the viewer's choice, saved in
//! this browser. `index.html` applies it before the first paint; this keeps
//! it in step afterwards (including System following the OS live).

use leptos::prelude::*;

const KEY: &str = "pedagog-theme";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    System,
    Light,
    Dark,
}

impl Choice {
    pub fn parse(saved: Option<&str>) -> Self {
        match saved {
            Some("light") => Self::Light,
            Some("dark") => Self::Dark,
            _ => Self::System,
        }
    }

    fn saved(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
        }
    }

    /// Whether this choice is dark, given the system's preference.
    pub fn dark(self, system_dark: bool) -> bool {
        match self {
            Self::System => system_dark,
            Self::Light => false,
            Self::Dark => true,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub choice: RwSignal<Choice>,
    system_dark: RwSignal<bool>,
}

impl Theme {
    pub fn is_dark(&self) -> bool {
        self.choice.get().dark(self.system_dark.get())
    }

    pub fn set(&self, choice: Choice) {
        self.choice.set(choice);
        if let Some(storage) = storage() {
            let _ = match choice.saved() {
                Some(v) => storage.set_item(KEY, v),
                None => storage.remove_item(KEY),
            };
        }
    }

    /// The top bar's toggle: to the other of light and dark.
    pub fn toggle(&self) {
        self.set(if self.is_dark() {
            Choice::Light
        } else {
            Choice::Dark
        });
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn system_query() -> Option<web_sys::MediaQueryList> {
    web_sys::window()?
        .match_media("(prefers-color-scheme: dark)")
        .ok()
        .flatten()
}

pub fn provide() -> Theme {
    let saved = storage().and_then(|s| s.get_item(KEY).ok().flatten());
    let theme = Theme {
        choice: RwSignal::new(Choice::parse(saved.as_deref())),
        system_dark: RwSignal::new(system_query().is_some_and(|q| q.matches())),
    };
    // Follow the system while the choice is System.
    if let Some(query) = system_query() {
        let system_dark = theme.system_dark;
        let on_change = wasm_bindgen::closure::Closure::<dyn Fn()>::new(move || {
            system_dark.set(system_query().is_some_and(|q| q.matches()));
        });
        let _ = query.add_event_listener_with_callback(
            "change",
            wasm_bindgen::JsCast::unchecked_ref(on_change.as_ref()),
        );
        on_change.forget();
    }
    Effect::new(move || {
        let dark = theme.is_dark();
        if let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        {
            let _ = root.class_list().toggle_with_force("dark", dark);
        }
    });
    provide_context(theme);
    theme
}

pub fn use_theme() -> Theme {
    expect_context::<Theme>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_follows_the_os_and_choices_override_it() {
        assert_eq!(Choice::parse(None), Choice::System);
        assert_eq!(Choice::parse(Some("dark")), Choice::Dark);
        assert_eq!(Choice::parse(Some("sepia")), Choice::System);
        assert!(Choice::System.dark(true) && !Choice::System.dark(false));
        assert!(!Choice::Light.dark(true) && Choice::Dark.dark(false));
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod browser {
    use wasm_bindgen_test::*;

    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    fn root_is_dark() -> bool {
        web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
            .is_some_and(|e| e.class_list().contains("dark"))
    }

    #[wasm_bindgen_test]
    async fn choices_apply_to_the_page_and_are_saved() {
        // Provided inside a mounted view, as the app does (mounting starts the
        // executor its effects run on, whichever test runs first).
        let slot = std::rc::Rc::new(std::cell::Cell::new(None));
        let set = slot.clone();
        crate::testing::mount(move || set.set(Some(provide())));
        let theme = slot.get().expect("provided");
        theme.set(Choice::Dark);
        crate::testing::settle(50).await;
        assert!(root_is_dark());
        assert_eq!(
            storage()
                .and_then(|s| s.get_item(KEY).ok().flatten())
                .as_deref(),
            Some("dark")
        );
        theme.set(Choice::Light);
        crate::testing::settle(50).await;
        assert!(!root_is_dark());
        theme.set(Choice::System);
        assert_eq!(
            storage().and_then(|s| s.get_item(KEY).ok().flatten()),
            None,
            "System isn't saved"
        );
    }
}

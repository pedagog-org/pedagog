//! Who's logged in (`GET /api/me`), loaded once and shared by every page;
//! reloaded after logging in or out.

use leptos::prelude::*;
use pedagog_core::api::{Me, MyCourse};

/// The session, as pages see it.
#[derive(Clone, Copy)]
pub struct Session {
    me: RwSignal<Option<Me>>,
    /// Whether `/api/me` has answered at least once.
    loaded: RwSignal<bool>,
}

impl Session {
    pub fn me(&self) -> Option<Me> {
        self.me.get()
    }

    pub fn loaded(&self) -> bool {
        self.loaded.get()
    }

    pub fn course(&self, id: &str) -> Option<MyCourse> {
        self.me
            .with(|me| me.as_ref()?.courses.iter().find(|c| c.id == id).cloned())
    }

    pub fn is_admin(&self) -> bool {
        self.me
            .with(|me| me.as_ref().is_some_and(|m| m.user.role == "admin"))
    }

    /// Ask the api again (after logging in, or a course change). Until it
    /// answers the session counts as unknown, so pages wait instead of sending
    /// someone who just logged in back to log in.
    pub fn reload(&self) {
        let (me, loaded) = (self.me, self.loaded);
        loaded.set(false);
        leptos::task::spawn_local(async move {
            let got = crate::api::get::<Me>("/me").await.ok();
            me.set(got);
            loaded.set(true);
        });
    }

    pub fn forget(&self) {
        self.me.set(None);
    }
}

/// Make the session available to the app (call once, at the root).
pub fn provide() -> Session {
    let session = Session {
        me: RwSignal::new(None),
        loaded: RwSignal::new(false),
    };
    provide_context(session);
    session.reload();
    session
}

pub fn use_session() -> Session {
    expect_context::<Session>()
}

/// The session ended (a 401): log in again, then come back here.
pub fn expired() {
    let Some(location) = web_sys::window().map(|w| w.location()) else {
        return;
    };
    let path = location.pathname().unwrap_or_default();
    if path.starts_with("/login") {
        return;
    }
    let here = format!("{path}{}", location.search().unwrap_or_default());
    let _ = location.assign(&crate::api::login_url(&here));
}

/// Two initials for an avatar: from the name, else the email.
pub fn initials(name: &str, email: &str) -> String {
    let from_name: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect();
    if from_name.is_empty() {
        email.chars().take(1).collect::<String>().to_uppercase()
    } else {
        from_name.to_uppercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_come_from_the_name_or_email() {
        assert_eq!(initials("Lena Park", "l@ufl.edu"), "LP");
        assert_eq!(initials("Ana María Ruiz", "a@ufl.edu"), "AM");
        assert_eq!(initials("", "bob@ufl.edu"), "B");
    }
}

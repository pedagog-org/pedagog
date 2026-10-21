//! `POST /api/client-errors`: the web UI reports what it didn't expect (a
//! panic, a reply it couldn't read). Signed-in users only, at most
//! [`PER_MINUTE`] a minute each, small bodies; logged as warnings, so they
//! reach GlitchTip beside the api's own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use pedagog_core::api::ClientError;

use crate::auth::CurrentUser;
use crate::error::{ApiError, refused};
use crate::state::AppState;

pub const PER_MINUTE: u32 = 20;
/// The largest report body accepted.
pub const MAX_BODY: usize = 16 << 10;

/// Reports per user in the current minute.
#[derive(Clone, Default)]
pub struct Limits(Arc<Mutex<HashMap<i64, (Instant, u32)>>>);

impl Limits {
    /// Count one report from `user`; false once they're over the limit.
    fn allow(&self, user: i64, now: Instant) -> bool {
        let Ok(mut seen) = self.0.lock() else {
            return false;
        };
        seen.retain(|_, (since, _)| now.duration_since(*since) < Duration::from_secs(60));
        let (_, n) = seen.entry(user).or_insert((now, 0));
        *n += 1;
        *n <= PER_MINUTE
    }
}

fn clip(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

pub async fn report(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(report): Json<ClientError>,
) -> Result<StatusCode, ApiError> {
    if !state.client_errors.allow(current.user.id, Instant::now()) {
        return Err(refused(
            StatusCode::TOO_MANY_REQUESTS,
            "too many error reports: try again in a minute",
        ));
    }
    tracing::warn!(
        source = "web",
        user = %current.user.email,
        path = clip(&report.path, 300),
        version = clip(&report.version, 60),
        message = clip(&report.message, 4000),
        "the web UI hit an error"
    );
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_limited_per_user_per_minute() {
        let limits = Limits::default();
        let t = Instant::now();
        assert!((0..PER_MINUTE).all(|_| limits.allow(1, t)));
        assert!(!limits.allow(1, t), "the 21st in a minute");
        assert!(limits.allow(2, t), "someone else");
        assert!(
            limits.allow(1, t + Duration::from_secs(61)),
            "a minute later"
        );
        assert_eq!(clip("héllo", 2), "hé");
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn reports_need_a_session_and_a_small_body(pool: sqlx::PgPool) -> sqlx::Result<()> {
        pedagog_store::db::accounts::upsert_user(&pool, "a@ufl.edu", "").await?;
        let (url, outbox) = crate::testing::serve(pool).await;
        let body = serde_json::json!({"path": "/courses/c", "message": "panicked", "version": "pedagog-web@0.0.0"});
        let anonymous =
            crate::testing::post(&format!("{url}/client-errors"), body.clone(), None).await;
        assert_eq!(anonymous.status(), 401);
        let token = crate::testing::login(&url, &outbox, "a@ufl.edu").await;
        let ok = crate::testing::post(&format!("{url}/client-errors"), body, Some(&token)).await;
        assert_eq!(ok.status(), 204);
        let huge =
            serde_json::json!({"path": "/", "message": "x".repeat(MAX_BODY), "version": "v"});
        let big = crate::testing::post(&format!("{url}/client-errors"), huge, Some(&token)).await;
        assert_eq!(big.status(), 413);
        Ok(())
    }
}

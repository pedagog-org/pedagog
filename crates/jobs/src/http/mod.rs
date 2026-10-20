//! HTTP surface: the dispatch-and-return rebuild endpoint, assignment uploads
//! and submissions.

pub mod error;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use serde::Deserialize;

use crate::dispatch::dispatch;
use crate::intake;
use crate::state::AppState;
use crate::submissions;

/// Room for multipart framing on top of a submission's content.
const MULTIPART_OVERHEAD: usize = 1 << 20;

pub fn router(state: AppState) -> Router {
    let max_upload = state.config.limits.max_compressed_bytes;
    let max_submission = usize::try_from(state.config.submission_limits.max_bytes)
        .unwrap_or(usize::MAX)
        .saturating_add(MULTIPART_OVERHEAD);
    Router::new()
        .route("/internal/recipes/rebuild", post(rebuild))
        .route("/internal/registry/cleanup", post(registry_cleanup))
        .route(
            "/internal/courses/{course}/assignments",
            post(intake::upload).layer(DefaultBodyLimit::max(max_upload)),
        )
        .route(
            "/internal/courses/{course}/assignments/{slug}/builds/{build}",
            get(intake::build_status),
        )
        .route(
            "/internal/courses/{course}/assignments/{slug}/submissions",
            post(submissions::submit).layer(DefaultBodyLimit::max(max_submission)),
        )
        .route(
            "/internal/submissions/{id}",
            get(submissions::submission_status),
        )
        .with_state(state)
}

#[derive(Deserialize)]
struct RebuildQuery {
    #[serde(default)]
    force: bool,
    os: Option<String>,
}

/// Runs a dispatch pass and returns immediately with what was kicked off —
/// not the build outcome (callers read `build_runs` / the live Job).
/// Run the registry cleanup now (operators; it also runs nightly), in the
/// background: its result is logged.
async fn registry_cleanup(State(state): State<AppState>) -> impl IntoResponse {
    tokio::spawn(async move {
        match crate::registry::cleanup(&state).await {
            Ok(c) => tracing::info!(?c, "registry cleanup done"),
            Err(e) => tracing::error!(error = %e, "registry cleanup failed"),
        }
    });
    StatusCode::ACCEPTED
}

async fn rebuild(
    State(state): State<AppState>,
    Query(q): Query<RebuildQuery>,
) -> impl IntoResponse {
    match dispatch(&state, q.force, q.os.as_deref()).await {
        Ok(d) => (StatusCode::ACCEPTED, Json(d)).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "dispatch failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

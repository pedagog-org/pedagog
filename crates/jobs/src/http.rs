//! HTTP surface — the dispatch-and-return rebuild endpoint.

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::post;
use serde::Deserialize;

use crate::dispatch::dispatch;
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/internal/recipes/rebuild", post(rebuild))
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

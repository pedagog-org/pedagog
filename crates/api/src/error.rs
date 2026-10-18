//! What the api answers when it refuses a request: a status and a message,
//! as `{"error": "..."}`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};

#[derive(Debug, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    pub error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.error })),
        )
            .into_response()
    }
}

pub fn refused(status: StatusCode, error: impl Into<String>) -> ApiError {
    ApiError {
        status,
        error: error.into(),
    }
}

pub fn unauthorized(error: impl Into<String>) -> ApiError {
    refused(StatusCode::UNAUTHORIZED, error)
}

/// A failure on our side (logged; the message names what failed).
pub fn internal(what: &str, e: impl std::fmt::Display) -> ApiError {
    tracing::error!(error = %e, "{what} failed");
    refused(StatusCode::INTERNAL_SERVER_ERROR, format!("{what} failed"))
}

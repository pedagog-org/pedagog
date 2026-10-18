//! What a handler answers when it refuses a request: a status and a message,
//! as `{"error": "..."}`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};

/// Why a request was refused, with its HTTP status.
#[derive(Debug, PartialEq, Eq)]
pub struct Refused {
    pub status: StatusCode,
    pub error: String,
}

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.error });
        (self.status, Json(body)).into_response()
    }
}

pub fn refused(status: StatusCode, error: impl Into<String>) -> Refused {
    Refused {
        status,
        error: error.into(),
    }
}

/// A failure on our side (logged; the message names what failed).
pub fn internal(what: &str, e: impl std::fmt::Display) -> Refused {
    tracing::error!(error = %e, "{what} failed");
    refused(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("{what} failed: {e}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_errors_say_what_failed() {
        let r = internal("storing the archive", "disk full");
        assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(r.error, "storing the archive failed: disk full");
    }
}

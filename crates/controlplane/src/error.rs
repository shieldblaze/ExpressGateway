//! Unified error types for the control plane.

/// Errors from the control plane store and REST API.
#[derive(Debug, thiserror::Error)]
pub enum ControlPlaneError {
    #[error("resource not found: {kind} with id '{id}'")]
    NotFound { kind: &'static str, id: String },

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("authentication error: {0}")]
    Auth(#[from] crate::auth::AuthError),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("internal error: {0}")]
    Internal(String),
}

impl ControlPlaneError {
    /// HTTP status code for this error.
    pub fn status_code(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Validation(_) => StatusCode::BAD_REQUEST,
            Self::Auth(crate::auth::AuthError::InvalidToken) => StatusCode::UNAUTHORIZED,
            Self::Auth(crate::auth::AuthError::RateLimited) => StatusCode::TOO_MANY_REQUESTS,
            Self::Serialization(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// JSON error response body.
#[derive(Debug, serde::Serialize)]
struct ErrorBody {
    error: String,
    code: u16,
}

impl axum::response::IntoResponse for ControlPlaneError {
    fn into_response(self) -> axum::response::Response {
        let status = self.status_code();
        let body = ErrorBody {
            error: self.to_string(),
            code: status.as_u16(),
        };
        (status, axum::Json(body)).into_response()
    }
}

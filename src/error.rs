//! Error types for MOM

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum MomError {
    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, MomError>;

impl IntoResponse for MomError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            MomError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            MomError::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg),
            MomError::SerializationError(e) => (StatusCode::BAD_REQUEST, e.to_string()),
            MomError::StorageError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            MomError::DatabaseError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            MomError::IoError(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            MomError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = serde_json::json!({
            "error": error_message,
        });

        (status, axum::Json(body)).into_response()
    }
}

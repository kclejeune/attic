//! Error types for the Attic Worker.

use std::fmt;

use serde::Serialize;
use worker::*;

/// Result type for worker operations.
pub type WorkerResult<T> = std::result::Result<T, WorkerError>;

/// Error types for the Attic Worker.
#[derive(Debug)]
pub enum WorkerError {
    /// Configuration error (missing env vars, invalid config).
    Configuration(String),

    /// Authentication error (invalid/missing token).
    Authentication(String),

    /// Authorization error (insufficient permissions).
    Authorization(String),

    /// Resource not found.
    NotFound(String),

    /// Bad request (invalid input).
    BadRequest(String),

    /// Storage error (R2 operations).
    Storage(String),

    /// Database error.
    Database(String),

    /// Compression error.
    Compression(String),

    /// Internal error.
    Internal(String),
}

impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkerError::Configuration(msg) => write!(f, "Configuration error: {}", msg),
            WorkerError::Authentication(msg) => write!(f, "Authentication error: {}", msg),
            WorkerError::Authorization(msg) => write!(f, "Authorization error: {}", msg),
            WorkerError::NotFound(msg) => write!(f, "Not found: {}", msg),
            WorkerError::BadRequest(msg) => write!(f, "Bad request: {}", msg),
            WorkerError::Storage(msg) => write!(f, "Storage error: {}", msg),
            WorkerError::Database(msg) => write!(f, "Database error: {}", msg),
            WorkerError::Compression(msg) => write!(f, "Compression error: {}", msg),
            WorkerError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for WorkerError {}

/// Error response body.
#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    message: String,
}

impl WorkerError {
    /// Convert to HTTP status code.
    pub fn status_code(&self) -> u16 {
        match self {
            WorkerError::Configuration(_) => 500,
            WorkerError::Authentication(_) => 401,
            WorkerError::Authorization(_) => 403,
            WorkerError::NotFound(_) => 404,
            WorkerError::BadRequest(_) => 400,
            WorkerError::Storage(_) => 502,
            WorkerError::Database(_) => 502,
            WorkerError::Compression(_) => 500,
            WorkerError::Internal(_) => 500,
        }
    }

    /// Convert to error type string.
    pub fn error_type(&self) -> &'static str {
        match self {
            WorkerError::Configuration(_) => "ConfigurationError",
            WorkerError::Authentication(_) => "AuthenticationError",
            WorkerError::Authorization(_) => "AuthorizationError",
            WorkerError::NotFound(_) => "NotFound",
            WorkerError::BadRequest(_) => "BadRequest",
            WorkerError::Storage(_) => "StorageError",
            WorkerError::Database(_) => "DatabaseError",
            WorkerError::Compression(_) => "CompressionError",
            WorkerError::Internal(_) => "InternalError",
        }
    }

    /// Convert to HTTP Response.
    pub fn to_response(&self) -> Response {
        let body = ErrorResponse {
            error: self.error_type().to_string(),
            message: self.to_string(),
        };

        let json = serde_json::to_string(&body).unwrap_or_else(|_| {
            format!(
                r#"{{"error":"{}","message":"{}"}}"#,
                self.error_type(),
                self.to_string()
            )
        });

        Response::from_json(&body)
            .unwrap_or_else(|_| Response::error(&json, self.status_code()).unwrap())
            .with_status(self.status_code())
    }
}

impl From<worker::Error> for WorkerError {
    fn from(e: worker::Error) -> Self {
        WorkerError::Internal(e.to_string())
    }
}

impl From<serde_json::Error> for WorkerError {
    fn from(e: serde_json::Error) -> Self {
        WorkerError::BadRequest(format!("JSON parse error: {}", e))
    }
}

impl From<WorkerError> for worker::Error {
    fn from(e: WorkerError) -> Self {
        worker::Error::RustError(e.to_string())
    }
}

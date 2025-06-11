use thiserror::Error;

use crate::types::error_types::Error as ApiError;

/// A unified error type for this library.
#[derive(Debug, Error)]
pub enum RevoltError {
    /// HTTP request failed (network or protocol issue).
    #[error("Reqwest Error: {0}")]
    ReqwestError(#[from] reqwest::Error),

    /// HTTP returned a non-2xx status, and we tried to parse the error body.
    /// In some cases we might have partial data.
    #[error("API Error: {0:?}")]
    ApiError(ApiError),

    /// The server returned an error code we didn't parse as `ApiError`.
    /// Contains the HTTP status code and raw body.
    #[error("Non-success HTTP status {code}, body: {body}")]
    HttpStatus {
        code: u16,
        body: String,
    },

    /// Serde (de)serialization error.
    #[error("Serde JSON error: {0}")]
    SerdeError(#[from] serde_json::Error),

    // Other
    #[error("Other error: {0}")]
    Other(String),
}

/// Convert an `Error` object from the schema into a `RevoltError::ApiError`.
pub fn handle_api_error(err: ApiError) -> RevoltError {
    RevoltError::ApiError(err)
}

//! API errors, serialized in the same shape as PocketBase:
//! `{"status": 400, "message": "...", "data": {"field": {"code": "...", "message": "..."}}}`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::superusers::FieldErrors;
use crate::validation::Errors;

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
    pub data: Map<String, Value>,
    /// The internal cause (logged, never returned; boxed to keep errors small).
    pub details: Option<Box<String>>,
}

/// Attached to error responses for the request log.
#[derive(Debug, Clone)]
pub struct ErrorInfo {
    pub message: String,
    pub details: Option<String>,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            data: Map::new(),
            details: None,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn validation(errors: FieldErrors) -> Self {
        let mut err = Self::bad_request("Failed to validate the submitted data.");
        for (field, message) in errors {
            err.data.insert(
                field.to_string(),
                json!({ "code": "validation_invalid", "message": message }),
            );
        }
        err
    }

    /// A 400 error with nested validation details.
    pub fn invalid_data(message: impl Into<String>, errors: &Errors) -> Self {
        let mut err = Self::bad_request(message);
        err.data = errors.to_json();
        err
    }

    pub fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "The requested resource wasn't found.",
        )
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    pub fn superusers_only() -> Self {
        Self::forbidden("Only superusers can perform this action.")
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, message)
    }

    pub fn internal(err: anyhow::Error) -> Self {
        let details = format!("{err:#}");
        tracing::error!(error = %details, "internal error");
        let mut error = Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong while processing your request.",
        );
        error.details = Some(Box::new(details));
        error
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({
            "status": self.status.as_u16(),
            "message": &self.message,
            "data": self.data,
        });
        let mut response = (self.status, Json(body)).into_response();
        response.extensions_mut().insert(ErrorInfo {
            message: self.message,
            details: self.details.map(|d| *d),
        });
        response
    }
}

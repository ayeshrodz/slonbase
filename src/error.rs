//! API errors, serialized in the same shape as PocketBase:
//! `{"status": 400, "message": "...", "data": {"field": {"code": "...", "message": "..."}}}`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::superusers::FieldErrors;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
    pub data: Map<String, Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            data: Map::new(),
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

    pub fn internal(err: anyhow::Error) -> Self {
        tracing::error!(error = %format!("{err:#}"), "internal error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong while processing your request.",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({
            "status": self.status.as_u16(),
            "message": self.message,
            "data": self.data,
        });
        (self.status, Json(body)).into_response()
    }
}

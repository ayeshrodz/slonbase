//! Request extractors with PocketBase-style JSON errors.

use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::http::header;
use serde_json::{Map, Value};

use crate::error::ApiError;

/// A JSON object request body. An empty body is treated as `{}`.
pub struct JsonObject(pub Map<String, Value>);

impl<S: Send + Sync> FromRequest<S> for JsonObject {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let content_type = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|e| ApiError::bad_request(format!("Failed to read the request body: {e}")))?;

        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self(Map::new()));
        }
        if !content_type.is_empty() && !content_type.starts_with("application/json") {
            return Err(ApiError::bad_request(
                "Unsupported Content-Type, only application/json is supported for now.",
            ));
        }
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(map)) => Ok(Self(map)),
            Ok(_) => Err(ApiError::bad_request(
                "The request body must be a JSON object.",
            )),
            Err(e) => Err(ApiError::bad_request(format!("Invalid JSON body: {e}"))),
        }
    }
}

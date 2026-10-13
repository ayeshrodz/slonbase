//! App settings (PocketBase's `meta`, `smtp` and `s3` sections), stored in `_slonbase.params`.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use deadpool_postgres::GenericClient;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::App;
use crate::auth::RequestAuth;
use crate::error::ApiError;
use crate::extract::JsonObject;

const PARAMS_KEY: &str = "settings";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub meta: Meta,
    pub smtp: Smtp,
    /// File storage in an S3 bucket instead of the local storage directory.
    pub s3: S3,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct S3 {
    pub enabled: bool,
    pub bucket: String,
    pub region: String,
    /// e.g. `https://s3.eu-west-1.amazonaws.com` or `http://minio:9000`.
    pub endpoint: String,
    pub access_key: String,
    /// Never returned by the API; an empty value on update keeps the stored one.
    pub secret: String,
    pub force_path_style: bool,
}

impl S3 {
    pub fn config(&self) -> crate::s3::Config {
        crate::s3::Config {
            endpoint: self.endpoint.trim().to_string(),
            bucket: self.bucket.trim().to_string(),
            region: self.region.trim().to_string(),
            access_key: self.access_key.trim().to_string(),
            secret: self.secret.clone(),
            force_path_style: self.force_path_style,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Meta {
    pub app_name: String,
    /// Public URL of the app, used in email links.
    #[serde(rename = "appURL")]
    pub app_url: String,
    pub sender_name: String,
    pub sender_address: String,
}

impl Default for Meta {
    fn default() -> Self {
        Self {
            app_name: "slonbase".into(),
            app_url: "http://localhost:8090".into(),
            sender_name: "Support".into(),
            sender_address: "support@example.com".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Smtp {
    /// When disabled, emails are written to the log instead of being sent.
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// Implicit TLS (usually port 465). Otherwise STARTTLS is used when offered.
    pub tls: bool,
}

impl Default for Smtp {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "smtp.example.com".into(),
            port: 587,
            username: String::new(),
            password: String::new(),
            tls: false,
        }
    }
}

impl Settings {
    pub async fn load(client: &impl GenericClient) -> anyhow::Result<Self> {
        let row = client
            .query_opt(
                "SELECT value FROM _slonbase.params WHERE key = $1",
                &[&PARAMS_KEY],
            )
            .await?;
        Ok(match row {
            Some(row) => serde_json::from_value(row.get(0))?,
            None => Self::default(),
        })
    }

    async fn save(&self, client: &impl GenericClient) -> anyhow::Result<()> {
        client
            .execute(
                "INSERT INTO _slonbase.params (key, value) VALUES ($1, $2)
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated = now()",
                &[&PARAMS_KEY, &serde_json::to_value(self)?],
            )
            .await?;
        Ok(())
    }

    /// The representation returned by the API (passwords and secrets are never sent back).
    fn public_json(&self) -> Value {
        let mut value = serde_json::to_value(self).expect("settings serialize");
        value["smtp"]["password"] = Value::String(String::new());
        value["s3"]["secret"] = Value::String(String::new());
        value
    }
}

fn validate(s: &Settings) -> crate::validation::Errors {
    let mut errors = crate::validation::Errors::new();
    if s.meta.app_name.trim().is_empty() {
        errors.add("meta.appName", "validation_required", "Cannot be blank.");
    }
    let url = s.meta.app_url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        errors.invalid("meta.appURL", "Must be a valid http(s) url.");
    }
    if !s.meta.sender_address.contains('@') {
        errors.invalid("meta.senderAddress", "Must be a valid email address.");
    }
    if s.smtp.enabled {
        if s.smtp.host.trim().is_empty() {
            errors.add("smtp.host", "validation_required", "Cannot be blank.");
        }
        if s.smtp.port == 0 {
            errors.invalid("smtp.port", "Must be a valid port.");
        }
    }
    if s.s3.enabled {
        for (key, value) in [
            ("s3.bucket", &s.s3.bucket),
            ("s3.region", &s.s3.region),
            ("s3.endpoint", &s.s3.endpoint),
            ("s3.accessKey", &s.s3.access_key),
            ("s3.secret", &s.s3.secret),
        ] {
            if value.trim().is_empty() {
                errors.add(key, "validation_required", "Cannot be blank.");
            }
        }
        if !s.s3.endpoint.trim().is_empty() && !s.s3.bucket.trim().is_empty() {
            if let Err(err) = crate::s3::Client::new(s.s3.config()) {
                errors.invalid("s3.endpoint", format!("{err:#}."));
            }
        }
    }
    errors
}

/// `GET /api/settings`
pub async fn get(State(app): State<Arc<App>>, auth: RequestAuth) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    Ok(Json(core.settings().public_json()))
}

/// `PATCH /api/settings`. Sections are merged key by key; an empty or missing SMTP
/// password or S3 secret keeps the stored one.
pub async fn update(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;

    let current = core.settings();
    let mut merged = serde_json::to_value(current.as_ref()).expect("settings serialize");
    for (section, value) in body {
        match (merged.get_mut(&section), value) {
            (Some(Value::Object(target)), Value::Object(patch)) => {
                for (k, v) in patch {
                    let secret = matches!(
                        (section.as_str(), k.as_str()),
                        ("smtp", "password") | ("s3", "secret")
                    );
                    let keep_password = secret && v.as_str().is_none_or(str::is_empty);
                    if !keep_password {
                        target.insert(k, v);
                    }
                }
            }
            (Some(_), _) => {
                return Err(ApiError::bad_request(format!(
                    "{section:?} must be an object."
                )));
            }
            (None, _) => {} // unknown sections are ignored
        }
    }
    let settings: Settings = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("Invalid settings: {e}.")))?;
    let errors = validate(&settings);
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(
            "Failed to update settings.",
            &errors,
        ));
    }

    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    settings.save(&client).await.map_err(ApiError::internal)?;
    crate::realtime::notify(&client, "settings")
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    core.set_settings(settings.clone());
    Ok(Json(settings.public_json()))
}

/// `POST /api/settings/test/s3` (`{"filesystem": "storage"}`): writes, reads and deletes a
/// test file with the saved S3 settings.
pub async fn test_s3(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;

    let core = app.core()?;
    auth.require_superuser()?;
    match body.get("filesystem").and_then(Value::as_str) {
        None | Some("storage") => {}
        Some(_) => {
            let mut errors = crate::validation::Errors::new();
            errors.invalid("filesystem", "Must be \"storage\".");
            return Err(ApiError::invalid_data(
                "An error occurred while validating the submitted data.",
                &errors,
            ));
        }
    }
    let settings = core.settings();
    if !settings.s3.enabled {
        return Err(ApiError::bad_request("S3 storage is not enabled."));
    }
    let storage = crate::storage::Storage::s3(settings.s3.config());
    if let Err(err) = storage.check().await {
        return Err(ApiError::bad_request(format!(
            "Failed to initialize the S3 storage. Raw error: {err:#}."
        )));
    }
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

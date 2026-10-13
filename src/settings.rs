//! App settings (PocketBase's `meta` and `smtp` sections), stored in `_slonbase.params`.

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

    /// The representation returned by the API (the SMTP password is never sent back).
    fn public_json(&self) -> Value {
        let mut value = serde_json::to_value(self).expect("settings serialize");
        value["smtp"]["password"] = Value::String(String::new());
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
    errors
}

/// `GET /api/settings`
pub async fn get(State(app): State<Arc<App>>, auth: RequestAuth) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    Ok(Json(core.settings().public_json()))
}

/// `PATCH /api/settings`. Sections are merged key by key; an empty or missing SMTP
/// password keeps the stored one.
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
                    let keep_password = section == "smtp"
                        && k == "password"
                        && v.as_str().is_none_or(str::is_empty);
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
    core.set_settings(settings.clone());
    Ok(Json(settings.public_json()))
}

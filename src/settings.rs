//! App settings, in PocketBase's format (`meta`, `smtp`, `s3`, `backups`, `logs`, `batch`,
//! `rateLimits`, `trustedProxy`, `superuserIPs`), stored in `_slonbase.params`.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use deadpool_postgres::GenericClient;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::App;
use crate::auth::RequestAuth;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::validation::Errors;

const PARAMS_KEY: &str = "settings";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub meta: Meta,
    pub smtp: Smtp,
    /// File storage in an S3 bucket instead of the local storage directory.
    pub s3: S3,
    pub backups: Backups,
    pub logs: Logs,
    pub batch: Batch,
    pub rate_limits: RateLimits,
    pub trusted_proxy: TrustedProxy,
    /// When not empty, superusers can only sign in and act from these IPs or ranges.
    #[serde(rename = "superuserIPs")]
    pub superuser_ips: Vec<String>,
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
    /// The dashboard's accent color.
    pub accent_color: String,
    /// Hides the collection and record controls in the dashboard.
    pub hide_controls: bool,
}

impl Default for Meta {
    fn default() -> Self {
        Self {
            app_name: "slonbase".into(),
            app_url: "http://localhost:8090".into(),
            sender_name: "Support".into(),
            sender_address: "support@example.com".into(),
            accent_color: "#1055c9".into(),
            hide_controls: false,
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
    /// `PLAIN` or `LOGIN` (empty: chosen automatically).
    pub auth_method: String,
    /// Implicit TLS (usually port 465). Otherwise STARTTLS is used when offered.
    pub tls: bool,
    /// The name sent in `HELO`/`EHLO` (empty: the machine's host name).
    pub local_name: String,
}

impl Default for Smtp {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "smtp.example.com".into(),
            port: 587,
            username: String::new(),
            password: String::new(),
            auth_method: String::new(),
            tls: false,
            local_name: String::new(),
        }
    }
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

    fn validate(&self, prefix: &str, errors: &mut Errors) {
        if !self.enabled {
            return;
        }
        for (key, value) in [
            ("bucket", &self.bucket),
            ("region", &self.region),
            ("endpoint", &self.endpoint),
            ("accessKey", &self.access_key),
            ("secret", &self.secret),
        ] {
            if value.trim().is_empty() {
                errors.add(
                    format!("{prefix}.{key}"),
                    "validation_required",
                    "Cannot be blank.",
                );
            }
        }
        if !self.endpoint.trim().is_empty() && !self.bucket.trim().is_empty() {
            if let Err(err) = crate::s3::Client::new(self.config()) {
                errors.invalid(format!("{prefix}.endpoint"), format!("{err:#}."));
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Backups {
    /// Automatic backups schedule (cron expression, empty: none).
    pub cron: String,
    /// Automatic backups to keep.
    pub cron_max_keep: u32,
    /// Where backups are stored (the local backups directory when disabled).
    pub s3: S3,
}

impl Default for Backups {
    fn default() -> Self {
        Self {
            cron: String::new(),
            cron_max_keep: 3,
            s3: S3::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Logs {
    pub max_data_size: i64,
    /// Days logs are kept (0: logging disabled).
    pub max_days: u32,
    pub min_level: i32,
    #[serde(rename = "logIP")]
    pub log_ip: bool,
    pub log_auth_id: bool,
}

impl Default for Logs {
    fn default() -> Self {
        Self {
            max_data_size: 0,
            max_days: 5,
            min_level: 0,
            log_ip: true,
            log_auth_id: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Batch {
    pub enabled: bool,
    pub max_requests: u32,
    /// Seconds.
    pub timeout: u64,
    /// Bytes (0: the default limit).
    pub max_body_size: u64,
}

impl Default for Batch {
    fn default() -> Self {
        Self {
            enabled: false,
            max_requests: 50,
            timeout: 3,
            max_body_size: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RateLimits {
    pub rules: Vec<RateLimitRule>,
    #[serde(rename = "excludedIPs")]
    pub excluded_ips: Vec<String>,
    pub enabled: bool,
}

impl Default for RateLimits {
    fn default() -> Self {
        let rule = |label: &str, duration, max_requests| RateLimitRule {
            label: label.into(),
            audience: String::new(),
            duration,
            max_requests,
        };
        Self {
            rules: vec![
                rule("*:auth", 3, 2),
                rule("*:create", 5, 20),
                rule("/api/batch", 1, 3),
                rule("/api/", 10, 300),
            ],
            excluded_ips: vec![],
            enabled: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RateLimitRule {
    /// A path prefix (`/api/`, `POST /api/files`) or `collection:action` (`*:auth`).
    pub label: String,
    /// `""` (everyone), `@guest` or `@auth`.
    pub audience: String,
    /// Seconds.
    pub duration: u64,
    pub max_requests: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TrustedProxy {
    /// Headers with the client IP set by a trusted proxy (e.g. `X-Forwarded-For`).
    pub headers: Vec<String>,
    #[serde(rename = "useLeftmostIP")]
    pub use_leftmost_ip: bool,
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
        value["backups"]["s3"]["secret"] = Value::String(String::new());
        value
    }

    /// Keeps the stored secrets that an update left empty.
    fn keep_secrets(&mut self, current: &Settings) {
        for (new, old) in [
            (&mut self.smtp.password, &current.smtp.password),
            (&mut self.s3.secret, &current.s3.secret),
            (&mut self.backups.s3.secret, &current.backups.s3.secret),
        ] {
            if new.is_empty() {
                new.clone_from(old);
            }
        }
    }
}

fn validate(s: &Settings) -> Errors {
    let mut errors = Errors::new();
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
    if !matches!(s.smtp.auth_method.as_str(), "" | "PLAIN" | "LOGIN") {
        errors.invalid("smtp.authMethod", "Must be PLAIN or LOGIN.");
    }
    s.s3.validate("s3", &mut errors);
    s.backups.s3.validate("backups.s3", &mut errors);
    if s.batch.enabled {
        if s.batch.max_requests == 0 {
            errors.invalid("batch.maxRequests", "Must be at least 1.");
        }
        if s.batch.timeout == 0 {
            errors.invalid("batch.timeout", "Must be at least 1.");
        }
    }
    for (i, rule) in s.rate_limits.rules.iter().enumerate() {
        let key = |name: &str| format!("rateLimits.rules.{i}.{name}");
        if rule.label.trim().is_empty() {
            errors.add(key("label"), "validation_required", "Cannot be blank.");
        }
        if !matches!(rule.audience.as_str(), "" | "@guest" | "@auth") {
            errors.invalid(key("audience"), "Must be empty, @guest or @auth.");
        }
        if rule.duration == 0 {
            errors.invalid(key("duration"), "Must be at least 1.");
        }
        if rule.max_requests == 0 {
            errors.invalid(key("maxRequests"), "Must be at least 1.");
        }
    }
    for (key, list) in [
        ("rateLimits.excludedIPs", &s.rate_limits.excluded_ips),
        ("superuserIPs", &s.superuser_ips),
    ] {
        for (i, ip) in list.iter().enumerate() {
            if !valid_ip_or_range(ip) {
                errors.invalid(
                    format!("{key}.{i}"),
                    "Must be an IP address or a CIDR range.",
                );
            }
        }
    }
    for (i, header) in s.trusted_proxy.headers.iter().enumerate() {
        if axum::http::HeaderName::from_bytes(header.trim().as_bytes()).is_err() {
            errors.invalid(format!("trustedProxy.headers.{i}"), "Invalid header name.");
        }
    }
    errors
}

/// `1.2.3.4`, `::1`, `10.0.0.0/8`, `fd00::/8`.
fn valid_ip_or_range(value: &str) -> bool {
    let (ip, bits) = match value.trim().split_once('/') {
        Some((ip, bits)) => (ip, Some(bits)),
        None => (value.trim(), None),
    };
    let Ok(ip) = ip.parse::<std::net::IpAddr>() else {
        return false;
    };
    let max = if ip.is_ipv4() { 32 } else { 128 };
    bits.is_none_or(|b| b.parse::<u8>().is_ok_and(|b| b <= max))
}

/// Merges `patch` into `target` like PocketBase does: objects key by key (recursively),
/// anything else replaced.
fn merge(target: &mut Value, patch: Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                match target.get_mut(&key) {
                    Some(existing) => merge(existing, value),
                    None => {
                        target.insert(key, value);
                    }
                }
            }
        }
        (target, patch) => *target = patch,
    }
}

/// `GET /api/settings`
pub async fn get(State(app): State<Arc<App>>, auth: RequestAuth) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    Ok(Json(core.settings().public_json()))
}

/// `PATCH /api/settings`. Sections are merged key by key; an empty or missing password or
/// secret keeps the stored one.
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
        let Some(target) = merged.get_mut(&section) else {
            continue; // unknown sections are ignored
        };
        if target.is_object() && !value.is_object() {
            return Err(ApiError::bad_request(format!(
                "{section:?} must be an object."
            )));
        }
        merge(target, value);
    }
    let mut settings: Settings = serde_json::from_value(merged)
        .map_err(|e| ApiError::bad_request(format!("Invalid settings: {e}.")))?;
    settings.keep_secrets(&current);
    let errors = validate(&settings);
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(
            "An error occurred while validating the submitted data.",
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

fn invalid_field(key: &str, message: &str) -> ApiError {
    let mut errors = Errors::new();
    errors.invalid(key, message);
    ApiError::invalid_data(
        "An error occurred while validating the submitted data.",
        &errors,
    )
}

/// `POST /api/settings/test/s3` (`{"filesystem": "storage" | "backups"}`): writes, reads
/// and deletes a test file with the saved S3 settings.
pub async fn test_s3(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let settings = core.settings();
    let s3 = match body.get("filesystem").and_then(Value::as_str) {
        None | Some("storage") => &settings.s3,
        Some("backups") => &settings.backups.s3,
        Some(_) => {
            return Err(invalid_field(
                "filesystem",
                "Must be \"storage\" or \"backups\".",
            ));
        }
    };
    if !s3.enabled {
        return Err(ApiError::bad_request("S3 storage is not enabled."));
    }
    let storage = crate::storage::Storage::s3(s3.config());
    if let Err(err) = storage.check().await {
        return Err(ApiError::bad_request(format!(
            "Failed to initialize the S3 storage. Raw error: {err:#}."
        )));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/settings/test/email` (`{"email", "template", "collection"}`): sends a
/// template's email with a placeholder token, and reports SMTP errors.
pub async fn test_email(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let str_of = |key: &str| body.get(key).and_then(Value::as_str).unwrap_or_default();
    let email = str_of("email").trim();
    if !email.contains('@') {
        return Err(invalid_field("email", "Must be a valid email address."));
    }
    let collection = match str_of("collection") {
        "" => crate::collections::model::SUPERUSERS_COLLECTION_NAME,
        name => name,
    };
    let registry = core.registry();
    let options = registry
        .get(collection)
        .and_then(|c| c.auth_options())
        .ok_or_else(|| invalid_field("collection", "Must be an auth collection."))?;
    let template = match str_of("template") {
        "verification" => &options.verification_template,
        "password-reset" => &options.reset_password_template,
        "email-change" => &options.confirm_email_change_template,
        "otp" => &options.otp.email_template,
        _ => {
            return Err(invalid_field(
                "template",
                "Must be verification, password-reset, email-change or otp.",
            ));
        }
    };
    let settings = core.settings();
    let fill = |text: &str| {
        crate::mail::render(
            &text
                .replace("{OTP}", "123456")
                .replace("{OTP_ID}", "OTP_ID"),
            &settings,
            "TEST_TOKEN",
        )
    };
    let message = crate::mail::Message {
        to: email.to_string(),
        subject: fill(&template.subject),
        html: fill(&template.body),
    };
    if let Err(err) = core.send_mail_now(message).await {
        return Err(ApiError::bad_request(format!(
            "Failed to send the test email. Raw error: {err:#}."
        )));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_match_pocketbase_shape() {
        let value = Settings::default().public_json();
        for key in [
            "meta",
            "smtp",
            "s3",
            "backups",
            "logs",
            "batch",
            "rateLimits",
            "trustedProxy",
            "superuserIPs",
        ] {
            assert!(value.get(key).is_some(), "{key}");
        }
        assert_eq!(value["rateLimits"]["rules"][0]["label"], "*:auth");
        assert_eq!(value["logs"]["logIP"], true);
        assert_eq!(value["backups"]["s3"]["secret"], "");
        assert!(validate(&Settings::default()).is_empty());
    }

    #[test]
    fn merges_recursively() {
        let mut target = json!({ "a": { "b": 1, "c": { "d": 2, "e": 3 } }, "list": [1, 2] });
        merge(
            &mut target,
            json!({ "a": { "c": { "d": 9 } }, "list": [3] }),
        );
        assert_eq!(
            target,
            json!({ "a": { "b": 1, "c": { "d": 9, "e": 3 } }, "list": [3] })
        );
    }

    #[test]
    fn ip_ranges() {
        for ok in ["1.2.3.4", "10.0.0.0/8", "::1", "fd00::/8", " 127.0.0.1 "] {
            assert!(valid_ip_or_range(ok), "{ok}");
        }
        for bad in ["", "1.2.3", "10.0.0.0/33", "x", "::1/129"] {
            assert!(!valid_ip_or_range(bad), "{bad}");
        }
    }
}

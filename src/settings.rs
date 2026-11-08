//! App settings, in PocketBase's format (`meta`, `smtp`, `s3`, `backups`, `logs`, `batch`,
//! `rateLimits`, `trustedProxy`, `superuserIPs`), stored in `_slonbase.params`.

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use deadpool_postgres::GenericClient;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::app::App;
use crate::auth::RequestAuth;
use crate::core::Core;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::hooks::{self, DataEvent};
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
    /// The time zone cron expressions are read in, for the automatic backups and the cron jobs of
    /// hooks (a slonbase setting; PocketBase always uses UTC).
    pub schedule: Schedule,
    pub logs: Logs,
    pub batch: Batch,
    pub rate_limits: RateLimits,
    pub trusted_proxy: TrustedProxy,
    /// When not empty, superusers can only sign in and act from these IPs or ranges.
    #[serde(rename = "superuserIPs")]
    pub superuser_ips: Vec<String>,
    /// The dashboard's sign-in page (a slonbase setting; PocketBase has none).
    pub login: Login,
    /// Who may call the API from a browser, how much it may upload, HTTPS and maintenance
    /// (a slonbase setting).
    pub security: Security,
    /// The HTML around every email's body (a slonbase setting).
    pub mail_layout: MailLayout,
    pub files: Files,
    /// Superusers' roles by id (none: owner). Changed with `/api/team`, never sent by
    /// `/api/settings` nor changed by it.
    pub superuser_roles: std::collections::BTreeMap<String, String>,
    /// API keys (their hashes). Like the roles, only `/api/team` shows and changes them.
    pub api_keys: Vec<crate::roles::ApiKey>,
}

/// Settings that only `/api/team` shows and changes.
const PRIVATE_SECTIONS: &[&str] = &["superuserRoles", "apiKeys"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MailLayout {
    pub enabled: bool,
    /// A whole HTML document with `{CONTENT}` where the email's body goes.
    pub html: String,
}

impl Default for MailLayout {
    fn default() -> Self {
        Self {
            enabled: true,
            html: crate::mail::DEFAULT_LAYOUT.into(),
        }
    }
}

/// An accent (a name or `#rrggbb`) as a color for emails: the named accents' light tone.
pub fn accent_hex(accent: &str) -> &str {
    match accent {
        "slate" => "#4f6478",
        "mauve" => "#7a5f86",
        "clay" => "#a2553b",
        "sand" => "#85693a",
        "teal" => "#1f7676",
        "ocean" => "#1f63a8",
        "indigo" => "#4c4fb3",
        "rose" => "#a8405e",
        "forest" => "#2f6b3f",
        "amber" => "#8f5c0e",
        "plum" => "#7d3b72",
        "graphite" => "#4b5563",
        a if a.len() == 7
            && a.starts_with('#')
            && a[1..].chars().all(|c| c.is_ascii_hexdigit())
            && !["#1055c9", "#4a5a72", "#5062a6"].contains(&a.to_ascii_lowercase().as_str()) =>
        {
            a
        }
        _ => "#4f7a6c",
    }
}

/// What the API accepts beyond rate limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Security {
    /// Origins (`https://app.example.com`) whose pages may call the API; `*` for any, as
    /// PocketBase allows; empty for none (browsers then only call it from its own address).
    pub allowed_origins: Vec<String>,
    /// The largest request body, in MB (file uploads, mostly). Backups uploads aren't limited.
    pub max_upload_mb: u32,
    /// Tells browsers to use HTTPS only (`Strict-Transport-Security`), when the application
    /// URL is https.
    pub hsts: bool,
    /// The API answers 503 to everyone but superusers (the dashboard keeps working).
    pub maintenance: bool,
    pub maintenance_message: String,
    /// How often each account may be sent each kind of auth email, in seconds.
    pub auth_email_interval: u32,
    /// Refuses password sign-ins for an identity from an address after too many failures.
    pub login_lockout: LoginLockout,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LoginLockout {
    /// Failed sign-ins allowed in the window (0: off).
    pub attempts: u32,
    /// The window, in minutes (1 to 60).
    pub minutes: u32,
}

/// The lockout's default: on since 2.4 (older installations are switched on once, see
/// [`upgrade_login_lockout`]).
pub const DEFAULT_LOCKOUT_ATTEMPTS: u32 = 10;

impl Default for LoginLockout {
    fn default() -> Self {
        Self {
            attempts: DEFAULT_LOCKOUT_ATTEMPTS,
            minutes: 15,
        }
    }
}

/// Uploaded files (a slonbase setting).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Files {
    /// The largest file, in bytes, for file fields without a size of their own (0: 5 MB).
    pub default_max_size: i64,
    /// Thumbnails are made when asked for (off: the original is served).
    pub thumbs: bool,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            default_max_size: 0,
            thumbs: true,
        }
    }
}

impl Default for Security {
    fn default() -> Self {
        Self {
            allowed_origins: vec!["*".into()],
            max_upload_mb: 128,
            hsts: false,
            maintenance: false,
            maintenance_message: String::new(),
            auth_email_interval: 60,
            login_lockout: LoginLockout::default(),
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
    /// The dashboard's accent: one of [`ACCENTS`] or a `#rrggbb` color.
    pub accent_color: String,
    /// Hides the collection and record controls in the dashboard.
    pub hide_controls: bool,
    /// Branding assets (ids in `_slonbase.assets`, empty for none): the logo on light and on
    /// dark backgrounds (the dashboard, its sign-in page and emails) and the browser icon.
    pub logo: String,
    pub logo_dark: String,
    pub favicon: String,
}

/// The named accents the dashboard knows (their colors are in its stylesheet).
pub const ACCENTS: &[&str] = &[
    "sage", "slate", "mauve", "clay", "sand", "teal", "ocean", "indigo", "rose", "forest", "amber",
    "plum", "graphite",
];

/// The dashboard's sign-in page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Login {
    /// The heading (empty: "Sign in").
    pub title: String,
    /// The line under it (empty: the application's name).
    pub subtitle: String,
    /// An asset shown above the form (empty: the application's logo).
    pub logo: String,
    /// `split` (the form beside a code panel), `centered` (a card) or `minimal` (the form).
    pub layout: String,
    /// `none`, `grid` or `asset` (the image in `background_asset`).
    pub background: String,
    pub background_asset: String,
    /// A notice shown above the form, as plain text.
    pub notice: String,
    /// Links under the form (`https:` or `mailto:`), at most three.
    pub footer_links: Vec<LoginLink>,
    /// "Powered by slonbase" under the form.
    pub show_powered_by: bool,
}

impl Default for Login {
    fn default() -> Self {
        Self {
            title: String::new(),
            subtitle: String::new(),
            logo: String::new(),
            layout: "split".into(),
            background: "none".into(),
            background_asset: String::new(),
            notice: String::new(),
            footer_links: Vec::new(),
            show_powered_by: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LoginLink {
    pub label: String,
    pub url: String,
}

impl Settings {
    /// The branding assets the settings use, with where each is used.
    pub fn asset_uses(&self) -> Vec<(&str, &'static str)> {
        [
            (self.meta.logo.as_str(), "the logo"),
            (self.meta.logo_dark.as_str(), "the logo on dark backgrounds"),
            (self.meta.favicon.as_str(), "the browser icon"),
            (self.login.logo.as_str(), "the sign-in page's logo"),
            (
                self.login.background_asset.as_str(),
                "the sign-in page's background",
            ),
        ]
        .into_iter()
        .filter(|(id, _)| !id.is_empty())
        .collect()
    }
}

impl Default for Meta {
    fn default() -> Self {
        Self {
            app_name: "slonbase".into(),
            app_url: "http://localhost:8090".into(),
            sender_name: "Support".into(),
            sender_address: "support@example.com".into(),
            accent_color: "sage".into(),
            hide_controls: false,
            logo: String::new(),
            logo_dark: String::new(),
            favicon: String::new(),
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
    /// For file storage: answer file downloads with a redirect to a short-lived presigned
    /// URL, so clients download straight from the bucket (which they must be able to reach).
    pub presigned_downloads: bool,
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
pub struct Schedule {
    /// `UTC` (the default, what PocketBase uses), `system` (the time zone the server's operating
    /// system is set to) or an IANA name (`Pacific/Auckland`). `0 3 * * *` runs at 03:00 on that
    /// zone's clocks.
    pub timezone: String,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            timezone: "UTC".into(),
        }
    }
}

/// The time zone the operating system is set to: `TZ`, else `/etc/timezone`, else where
/// `/etc/localtime` points. UTC when none says (a minimal container image has none).
pub fn system_timezone() -> chrono_tz::Tz {
    let from_env = std::env::var("TZ").ok();
    let from_file = std::fs::read_to_string("/etc/timezone").ok();
    let from_link = std::fs::read_link("/etc/localtime").ok().and_then(|p| {
        p.to_str()
            .and_then(|s| s.split("zoneinfo/").nth(1))
            .map(String::from)
    });
    [from_env, from_file, from_link]
        .into_iter()
        .flatten()
        .map(|n| n.trim().trim_start_matches(':').to_string())
        .find_map(|n| n.parse().ok())
        .unwrap_or(chrono_tz::Tz::UTC)
}

/// A time zone by its IANA name, `system` (the OS's zone) or empty (UTC); `None` when it isn't one.
pub fn parse_timezone(name: &str) -> Option<chrono_tz::Tz> {
    if name.is_empty() {
        return Some(chrono_tz::Tz::UTC);
    }
    if name == "system" {
        return Some(system_timezone());
    }
    if name.len() > 64 {
        return None;
    }
    name.parse().ok()
}

impl Settings {
    /// The time zone schedules are read in (UTC when the setting isn't a valid name).
    pub fn timezone(&self) -> chrono_tz::Tz {
        parse_timezone(&self.schedule.timezone).unwrap_or(chrono_tz::Tz::UTC)
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

/// The largest integer JSON clients can read exactly (2^53 - 1).
const MAX_SAFE_JSON_INT: i64 = (1 << 53) - 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Logs {
    /// Largest serialized `data` of an entry, in bytes (0: 16 KiB); larger entries keep their
    /// leading attributes and get `"__pb_truncated__": true`.
    pub max_data_size: i64,
    /// Days logs are kept (0: logging disabled).
    pub max_days: u32,
    pub min_level: i32,
    #[serde(rename = "logIP")]
    pub log_ip: bool,
    pub log_auth_id: bool,
    /// Requests to paths starting with one of these aren't logged unless they fail with a
    /// server error (a slonbase setting).
    pub exclude_paths: Vec<String>,
    /// Requests slower than this many milliseconds are logged as warnings, whatever the
    /// minimum level (0: off; a slonbase setting).
    pub slow_request_ms: u32,
    /// At most this many entries are kept, the oldest deleted first (0: no limit; a
    /// slonbase setting).
    pub max_rows: u32,
}

impl Default for Logs {
    fn default() -> Self {
        Self {
            max_data_size: 0,
            max_days: 5,
            min_level: 0,
            log_ip: true,
            log_auth_id: false,
            exclude_paths: Vec::new(),
            slow_request_ms: 0,
            max_rows: 0,
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
    /// The addresses or ranges of the proxies allowed to set those headers (`10.0.0.0/8`,
    /// `192.168.1.5`). When there are some, the headers are believed only on connections from
    /// them, and trusted proxies in a list are skipped from the right. Empty: the headers are
    /// believed from any connection (only safe when nothing but the proxy can reach slonbase).
    #[serde(default)]
    pub peers: Vec<String>,
}

impl Settings {
    /// Loads the settings, decrypting them with `key` when they're stored encrypted (see
    /// [`crate::secrets`]).
    pub async fn load(
        client: &impl GenericClient,
        key: Option<&crate::secrets::SecretsKey>,
    ) -> anyhow::Result<Self> {
        let row = client
            .query_opt(
                "SELECT value FROM _slonbase.params WHERE key = $1",
                &[&PARAMS_KEY],
            )
            .await?;
        Ok(match row {
            Some(row) => {
                serde_json::from_value(crate::secrets::from_stored(row.get(0), key, "settings")?)?
            }
            None => Self::default(),
        })
    }

    /// Saves the settings of a new installation: the defaults, with rate limits on.
    /// Settings saved before (by a migration, say) are kept.
    pub(crate) async fn init(client: &impl GenericClient) -> anyhow::Result<()> {
        let mut settings = Self::default();
        settings.rate_limits.enabled = true;
        client
            .execute(
                "INSERT INTO _slonbase.params (key, value) VALUES ($1, $2) ON CONFLICT (key) DO NOTHING",
                &[&PARAMS_KEY, &serde_json::to_value(&settings)?],
            )
            .await?;
        Ok(())
    }

    /// Saves the settings, encrypted with `key` if there's one.
    pub(crate) async fn save(
        &self,
        client: &impl GenericClient,
        key: Option<&crate::secrets::SecretsKey>,
    ) -> anyhow::Result<()> {
        let value = crate::secrets::to_stored(&serde_json::to_value(self)?, key);
        client
            .execute(
                "INSERT INTO _slonbase.params (key, value) VALUES ($1, $2)
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated = now()",
                &[&PARAMS_KEY, &value],
            )
            .await?;
        Ok(())
    }

    /// The representation returned by the API (passwords and secrets are never sent back).
    pub(crate) fn public_json(&self) -> Value {
        let mut value = serde_json::to_value(self).expect("settings serialize");
        if let Some(o) = value.as_object_mut() {
            for key in PRIVATE_SECTIONS {
                o.remove(*key);
            }
        }
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
    if s.meta.app_name.chars().count() > 60 {
        errors.invalid("meta.appName", "Must be at most 60 characters.");
    }
    let accent = s.meta.accent_color.as_str();
    let hex = accent.len() == 7
        && accent.starts_with('#')
        && accent[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !(accent.is_empty() || hex || ACCENTS.contains(&accent)) {
        errors.invalid(
            "meta.accentColor",
            "Must be a named accent or a #rrggbb color.",
        );
    }
    for (key, id) in [
        ("meta.logo", &s.meta.logo),
        ("meta.logoDark", &s.meta.logo_dark),
        ("meta.favicon", &s.meta.favicon),
        ("login.logo", &s.login.logo),
        ("login.backgroundAsset", &s.login.background_asset),
    ] {
        if !(id.is_empty() || crate::assets::valid_id(id)) {
            errors.invalid(key, "Must be an asset's id.");
        }
    }
    validate_login(&s.login, &mut errors);
    if s.mail_layout.enabled && !s.mail_layout.html.contains("{CONTENT}") {
        errors.invalid(
            "mailLayout.html",
            "Must contain the {CONTENT} placeholder (where each email's body goes).",
        );
    }
    if s.mail_layout.html.len() > 100_000 {
        errors.invalid("mailLayout.html", "Must be at most 100 KB.");
    }
    if s.logs.exclude_paths.len() > 30 {
        errors.invalid("logs.excludePaths", "At most 30 paths.");
    }
    for (i, p) in s.logs.exclude_paths.iter().enumerate() {
        if !p.starts_with('/') || p.len() > 200 {
            errors.invalid(
                format!("logs.excludePaths.{i}"),
                "Must be a path starting with /.",
            );
        }
    }
    let sec = &s.security;
    if !(1..=16384).contains(&sec.max_upload_mb) {
        errors.invalid("security.maxUploadMb", "Must be between 1 and 16384.");
    }
    if !(5..=86_400).contains(&sec.auth_email_interval) {
        errors.invalid(
            "security.authEmailInterval",
            "Must be between 5 and 86400 seconds.",
        );
    }
    if sec.login_lockout.attempts > 1000 {
        errors.invalid("security.loginLockout.attempts", "Must be at most 1000.");
    }
    if sec.login_lockout.attempts > 0 && !(1..=60).contains(&sec.login_lockout.minutes) {
        errors.invalid("security.loginLockout.minutes", "Must be between 1 and 60.");
    }
    if !(0..=MAX_SAFE_JSON_INT).contains(&s.files.default_max_size) {
        errors.invalid("files.defaultMaxSize", "Must be 0 or more.");
    }
    if sec.maintenance_message.chars().count() > 300 {
        errors.invalid(
            "security.maintenanceMessage",
            "Must be at most 300 characters.",
        );
    }
    if sec.allowed_origins.len() > 50 {
        errors.invalid("security.allowedOrigins", "At most 50 origins.");
    }
    for (i, origin) in sec.allowed_origins.iter().enumerate() {
        if !(origin == "*" || crate::http_policy::valid_origin(origin)) {
            errors.invalid(
                format!("security.allowedOrigins.{i}"),
                "Must be * or an origin such as https://app.example.com (no path).",
            );
        }
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
    if !(0..=MAX_SAFE_JSON_INT).contains(&s.logs.max_data_size) {
        errors.invalid(
            "logs.maxDataSize",
            format!("Must be between 0 and {MAX_SAFE_JSON_INT}."),
        );
    }
    if !s.backups.cron.trim().is_empty() && crate::cron::Schedule::parse(&s.backups.cron).is_none()
    {
        errors.invalid("backups.cron", "Invalid cron expression.");
    }
    if parse_timezone(&s.schedule.timezone).is_none() {
        errors.invalid(
            "schedule.timezone",
            "Unknown time zone. Use UTC, system, or a name like Pacific/Auckland.",
        );
    }
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
            if crate::ip::Range::parse(ip).is_none() {
                errors.invalid(
                    format!("{key}.{i}"),
                    "Must be an IP address or a CIDR range.",
                );
            }
        }
    }
    for (i, peer) in s.trusted_proxy.peers.iter().enumerate() {
        if crate::ip::Range::parse(peer).is_none() {
            errors.invalid(
                format!("trustedProxy.peers.{i}"),
                "Must be an IP address or a CIDR range.",
            );
        }
    }
    for (i, header) in s.trusted_proxy.headers.iter().enumerate() {
        if axum::http::HeaderName::from_bytes(header.trim().as_bytes()).is_err() {
            errors.invalid(format!("trustedProxy.headers.{i}"), "Invalid header name.");
        }
    }
    errors
}

fn validate_login(l: &Login, errors: &mut Errors) {
    let too_long = |text: &str, max: usize| text.chars().count() > max;
    if too_long(&l.title, 80) {
        errors.invalid("login.title", "Must be at most 80 characters.");
    }
    if too_long(&l.subtitle, 160) {
        errors.invalid("login.subtitle", "Must be at most 160 characters.");
    }
    if too_long(&l.notice, 500) {
        errors.invalid("login.notice", "Must be at most 500 characters.");
    }
    if !matches!(l.layout.as_str(), "split" | "centered" | "minimal") {
        errors.invalid("login.layout", "Must be split, centered or minimal.");
    }
    if !matches!(l.background.as_str(), "none" | "grid" | "asset") {
        errors.invalid("login.background", "Must be none, grid or asset.");
    }
    if l.background == "asset" && l.background_asset.is_empty() {
        errors.add(
            "login.backgroundAsset",
            "validation_required",
            "Choose an image.",
        );
    }
    if l.footer_links.len() > 3 {
        errors.invalid("login.footerLinks", "At most 3 links.");
    }
    for (i, link) in l.footer_links.iter().enumerate() {
        let label = link.label.trim();
        if label.is_empty() || too_long(label, 40) {
            errors.invalid(
                format!("login.footerLinks.{i}.label"),
                "Must be 1 to 40 characters.",
            );
        }
        if !safe_link(&link.url) {
            errors.invalid(
                format!("login.footerLinks.{i}.url"),
                "Must be an https: or mailto: link.",
            );
        }
    }
}

/// An `https:` URL or a `mailto:` address, without spaces or control characters.
fn safe_link(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("mailto:"));
    rest.is_some_and(|r| !r.is_empty())
        && url.len() <= 500
        && !url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '<' || c == '>')
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
pub async fn get(
    State(app): State<Arc<App>>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    auth: RequestAuth,
    headers: axum::http::HeaderMap,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let request = request_info(
        &core,
        &auth,
        &axum::http::Method::GET,
        &query,
        &headers,
        None,
    );
    let e = DataEvent {
        core: core.clone(),
        collection: None,
        auth,
        request: Some(request),
        data: hooks::data([("settings", core.settings().public_json())]),
        response: None,
    };
    let listed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let e = {
        let listed = listed.clone();
        core.hooks
            .on_settings_list_request
            .trigger(None, e, move |e| async move {
                listed.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(e)
            })
            .await?
    };
    Ok(match e.response {
        Some(response) => response,
        None if listed.load(std::sync::atomic::Ordering::SeqCst) => {
            Json(e.data["settings"].clone()).into_response()
        }
        None => StatusCode::OK.into_response(),
    })
}

/// `PATCH /api/settings`. Sections are merged key by key; an empty or missing password or
/// secret keeps the stored one.
pub async fn update(
    State(app): State<Arc<App>>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    auth: RequestAuth,
    headers: axum::http::HeaderMap,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let old = core.settings().public_json();
    // the settings with the request applied, before validation (like PocketBase's)
    let mut new = old.clone();
    for (section, value) in &body {
        if let Some(target) = new.get_mut(section) {
            merge(target, value.clone());
        }
    }
    let request = request_info(
        &core,
        &auth,
        &axum::http::Method::PATCH,
        &query,
        &headers,
        Some(&body),
    );
    let e = DataEvent {
        core: core.clone(),
        collection: None,
        auth,
        request: Some(request),
        data: hooks::data([("oldSettings", old), ("newSettings", new)]),
        response: None,
    };
    let saved: Arc<Mutex<Option<Settings>>> = Default::default();
    let e = {
        let saved = saved.clone();
        core.hooks
            .clone()
            .on_settings_update_request
            .trigger(None, e, move |e| async move {
                let patch = match &e.data["newSettings"] {
                    Value::Object(patch) => patch.clone(),
                    _ => return Err(ApiError::bad_request("newSettings must be an object.")),
                };
                let settings = apply_patch(&e.core, patch).await?;
                *saved.lock().expect("settings lock") = Some(settings);
                Ok(e)
            })
            .await?
    };
    if let Some(response) = e.response {
        return Ok(response);
    }
    Ok(match saved.lock().expect("settings lock").take() {
        Some(settings) => Json(settings.public_json()).into_response(),
        None => StatusCode::OK.into_response(),
    })
}

/// The request as hooks see it.
fn request_info(
    core: &Core,
    auth: &RequestAuth,
    method: &axum::http::Method,
    query: &std::collections::HashMap<String, String>,
    headers: &axum::http::HeaderMap,
    body: Option<&Map<String, Value>>,
) -> Arc<crate::filter::RequestInfo> {
    Arc::new(crate::records::api::request_info(
        method,
        query,
        headers,
        body,
        auth,
        &core.registry(),
    ))
}

/// Merges `body` (sections of settings) into the current settings, validates and saves them
/// and tells the other instances.
pub(crate) async fn apply_patch(
    core: &Arc<Core>,
    body: Map<String, Value>,
) -> Result<Settings, ApiError> {
    let settings = prepare_patch(&core.settings(), body)?;
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    crate::assets::check_uses(&client, &settings).await?;
    settings
        .save(&client, core.secrets_key())
        .await
        .map_err(ApiError::internal)?;
    crate::realtime::notify(&client, "settings")
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    core.set_settings(settings.clone());
    reloaded(core);
    Ok(settings)
}

/// Runs the `on_settings_reload` handlers (in the background) after the settings changed.
pub(crate) fn reloaded(core: &Arc<Core>) {
    if !core.hooks.on_settings_reload.has_handlers(None) {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        let e = DataEvent::new(
            &core,
            hooks::data([("settings", core.settings().public_json())]),
        );
        if let Err(err) = core
            .hooks
            .on_settings_reload
            .trigger(None, e, |e| async { Ok(e) })
            .await
        {
            tracing::error!(error = %err.details.as_deref().unwrap_or(&err.message), "an onSettingsReload handler failed");
        }
    });
}

/// `current` with the sections of `body` merged in, validated.
pub(crate) fn prepare_patch(
    current: &Settings,
    body: Map<String, Value>,
) -> Result<Settings, ApiError> {
    let mut merged = serde_json::to_value(current).expect("settings serialize");
    for (section, value) in body {
        if PRIVATE_SECTIONS.contains(&section.as_str()) {
            continue;
        }
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
    settings.keep_secrets(current);
    let errors = validate(&settings);
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(
            "An error occurred while validating the submitted data.",
            &errors,
        ));
    }
    Ok(settings)
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

/// Longest validity of an Apple client secret (about 6 months, Apple's limit).
const APPLE_SECRET_MAX_DURATION: i64 = 15_777_000;

/// `POST /api/settings/apple/generate-client-secret` (`{"clientId", "teamId", "keyId",
/// "privateKey", "duration"}`): an Apple client secret, `{"secret"}`, as PocketBase's.
pub async fn generate_apple_client_secret(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    app.core()?;
    auth.require_superuser()?;
    let str_of = |key: &str| body.get(key).and_then(Value::as_str).unwrap_or_default();
    let (client_id, team_id, key_id, private_key) = (
        str_of("clientId"),
        str_of("teamId"),
        str_of("keyId"),
        str_of("privateKey"),
    );
    let duration = body.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
    let mut errors = Errors::new();
    for (key, value) in [
        ("clientId", client_id),
        ("teamId", team_id),
        ("keyId", key_id),
        ("privateKey", private_key),
    ] {
        if value.is_empty() {
            errors.add(key, "validation_required", "Cannot be blank.");
        } else if (key == "teamId" || key == "keyId") && value.chars().count() != 10 {
            errors.add(
                key,
                "validation_length_invalid",
                "The length must be exactly 10.",
            );
        } else if key == "privateKey"
            && !(value.contains("-----BEGIN PRIVATE KEY----")
                && value.contains("-----END PRIVATE KEY-----"))
        {
            errors.add(
                key,
                "validation_match_invalid",
                "Must be in a valid format.",
            );
        }
    }
    if duration == 0.0 {
        errors.add("duration", "validation_required", "Cannot be blank.");
    } else if duration < 1.0 {
        errors.add(
            "duration",
            "validation_min_greater_equal_than_required",
            "Must be no less than 1.",
        );
    } else if duration > APPLE_SECRET_MAX_DURATION as f64 {
        errors.add(
            "duration",
            "validation_max_less_equal_than_required",
            format!("Must be no greater than {APPLE_SECRET_MAX_DURATION}."),
        );
    }
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(
            "Invalid client secret data.",
            &errors,
        ));
    }
    let secret = crate::oauth2::apple_client_secret(
        client_id,
        team_id,
        key_id,
        private_key,
        duration as i64,
        chrono::Utc::now().timestamp(),
    )
    .map_err(|e| {
        ApiError::bad_request(format!(
            "Failed to generate client secret. Raw error: \n{e:#}"
        ))
    })?;
    Ok(Json(serde_json::json!({ "secret": secret })).into_response())
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
        "login-alert" => &options.auth_alert.email_template,
        _ => {
            return Err(invalid_field(
                "template",
                "Must be verification, password-reset, email-change, otp or login-alert.",
            ));
        }
    };
    let settings = core.settings();
    let (subject, html) = sample_mail(&settings, &template.subject, &template.body);
    let message = crate::mail::Message {
        to: email.to_string(),
        subject,
        html,
    };
    if let Err(err) = core.send_mail_now(message).await {
        return Err(ApiError::bad_request(format!(
            "Failed to send the test email. Raw error: {err:#}."
        )));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// A template filled with sample values (for the test email and the preview).
fn sample_mail(settings: &Settings, subject: &str, body: &str) -> (String, String) {
    let sample = |text: &str| {
        text.replace("{OTP}", "123456")
            .replace("{OTP_ID}", "OTP_ID")
            .replace(
                "{ALERT_INFO}",
                &crate::records::auth_api::alert_info("127.0.0.1", "Test client"),
            )
    };
    crate::mail::compose(
        settings,
        &sample(subject),
        &sample(body),
        "TEST_TOKEN",
        None,
    )
}

/// `POST /api/settings/mail/preview`: an email as it would be sent, with sample values.
/// `{collection, template}` picks a collection's template; `subject`, `body` and `layout`
/// (the draft being edited) stand in for the saved ones.
pub async fn mail_preview(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    let str_of = |key: &str| body.get(key).and_then(Value::as_str);
    let mut settings = (*core.settings()).clone();
    if let Some(layout) = body.get("layout").and_then(Value::as_object) {
        if let Some(enabled) = layout.get("enabled").and_then(Value::as_bool) {
            settings.mail_layout.enabled = enabled;
        }
        if let Some(html) = layout.get("html").and_then(Value::as_str) {
            settings.mail_layout.html = html.chars().take(100_000).collect();
        }
    }
    let (mut subject, mut text) = (String::new(), String::new());
    if let (Some(collection), Some(template)) = (str_of("collection"), str_of("template")) {
        let registry = core.registry();
        let options = registry
            .get(collection)
            .and_then(|c| c.auth_options())
            .ok_or_else(|| invalid_field("collection", "Must be an auth collection."))?;
        let t = match template {
            "verification" => &options.verification_template,
            "password-reset" => &options.reset_password_template,
            "email-change" => &options.confirm_email_change_template,
            "otp" => &options.otp.email_template,
            "login-alert" => &options.auth_alert.email_template,
            _ => return Err(invalid_field("template", "Unknown template.")),
        };
        subject.clone_from(&t.subject);
        text.clone_from(&t.body);
    }
    if let Some(s) = str_of("subject") {
        subject = s.chars().take(1000).collect();
    }
    if let Some(b) = str_of("body") {
        text = b.chars().take(200_000).collect();
    }
    let (subject, html) = sample_mail(&settings, &subject, &text);
    Ok(Json(
        serde_json::json!({ "subject": subject, "html": html }),
    ))
}

/// Turns the sign-in lockout on for an installation that has it off, once: migration 19 leaves
/// a marker, and this applies it and removes the marker (in one transaction, so a failure
/// leaves the marker for the next start). The settings may be encrypted, which is why it isn't
/// done in SQL.
pub(crate) async fn upgrade_login_lockout(
    pool: &deadpool_postgres::Pool,
    key: Option<&crate::secrets::SecretsKey>,
) -> anyhow::Result<()> {
    let mut client = pool.get().await.map_err(crate::db::describe_pool_error)?;
    let tx = client.transaction().await?;
    let marker = tx
        .query_opt(
            "SELECT 1 FROM _slonbase.params WHERE key = 'upgrade_login_lockout' FOR UPDATE",
            &[],
        )
        .await?;
    if marker.is_none() {
        return Ok(());
    }
    let stored = tx
        .query_opt(
            "SELECT 1 FROM _slonbase.params WHERE key = $1",
            &[&PARAMS_KEY],
        )
        .await?
        .is_some();
    if stored {
        let mut settings = Settings::load(&tx, key).await?;
        if settings.security.login_lockout.attempts == 0 {
            settings.security.login_lockout.attempts = DEFAULT_LOCKOUT_ATTEMPTS;
            if !(1..=60).contains(&settings.security.login_lockout.minutes) {
                settings.security.login_lockout.minutes = 15;
            }
            settings.save(&tx, key).await?;
            tracing::info!(
                "the sign-in lockout is now on ({DEFAULT_LOCKOUT_ATTEMPTS} failed sign-ins per identity and address in {} minutes); turn it off in Settings, Security",
                settings.security.login_lockout.minutes
            );
        }
    }
    tx.execute(
        "DELETE FROM _slonbase.params WHERE key = 'upgrade_login_lockout'",
        &[],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn system_time_zone_is_accepted_and_resolves() {
        // the name `system` is valid, whatever the OS says, and resolves to some zone
        assert!(super::parse_timezone("system").is_some());
        assert!(super::parse_timezone("System").is_none());
        assert!(super::parse_timezone("Pacific/Auckland ").is_none());
        let mut s = super::Settings::default();
        s.schedule.timezone = "system".into();
        let _ = s.timezone();
    }

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
}

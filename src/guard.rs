//! Request guards from the settings: rate limits (`rateLimits`) and the superuser IP
//! allowlist (`superuserIPs`), compatible with PocketBase.
//!
//! Rate limit rules match, in order, `{collection}:{action}` and `*:{action}` labels (for
//! collection endpoints, e.g. `users:auth`, `*:create`), then `METHOD /path` and `/path`
//! exactly, then labels ending in `/` as path prefixes (`/api/`). Each rule allows
//! `maxRequests` per `duration` seconds per client IP, in fixed windows. Superusers and
//! `excludedIPs` aren't limited. Limits are kept in memory, per instance.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::app::App;
use crate::auth::{RequestAuth, authenticate, token_from_header};
use crate::collections::model::SUPERUSERS_COLLECTION_ID;
use crate::collections::store::Registry;
use crate::core::Core;
use crate::error::ApiError;
use crate::settings::{RateLimitRule, RateLimits};

/// The client's IP (from the connection or a trusted proxy header), for handlers.
#[derive(Debug, Clone, Copy)]
pub struct ClientIp(pub Option<IpAddr>);

/// The caller, resolved by the guard so the auth extractor doesn't verify the token again.
#[derive(Debug, Clone)]
pub struct ResolvedAuth(pub RequestAuth);

/// Whether a superuser may act from `ip` (always, when the allowlist is empty).
pub fn superuser_ip_allowed(allowlist: &[String], ip: Option<IpAddr>) -> bool {
    allowlist.is_empty() || ip.is_some_and(|ip| crate::ip::in_any(allowlist, ip))
}

/// The actions of collection endpoints, as rate limit tags (PocketBase's names).
fn collection_tags(method: &Method, rest: &[&str]) -> &'static [&'static str] {
    match (method.as_str(), rest) {
        ("GET", ["records"]) => &["list"],
        ("POST", ["records"]) => &["create"],
        ("GET", ["records", _]) => &["view"],
        ("PATCH", ["records", _]) => &["update"],
        ("DELETE", ["records", _]) => &["delete"],
        ("GET", ["auth-methods"]) => &["listAuthMethods"],
        ("POST", ["auth-refresh"]) => &["authRefresh"],
        ("POST", ["auth-with-password"]) => &["authWithPassword", "auth"],
        ("POST", ["auth-with-oauth2"]) => &["authWithOAuth2", "auth"],
        ("POST", ["auth-with-otp"]) => &["authWithOTP", "auth"],
        ("POST", ["request-otp"]) => &["requestOTP"],
        ("POST", ["request-password-reset"]) => &["requestPasswordReset"],
        ("POST", ["confirm-password-reset"]) => &["confirmPasswordReset"],
        ("POST", ["request-verification"]) => &["requestVerification"],
        ("POST", ["confirm-verification"]) => &["confirmVerification"],
        ("POST", ["request-email-change"]) => &["requestEmailChange"],
        ("POST", ["confirm-email-change"]) => &["confirmEmailChange"],
        _ => &[],
    }
}

/// The rule labels of a request, most specific first, and the limiter key prefix.
fn labels(method: &Method, path: &str, registry: &Registry) -> (Vec<String>, Option<String>) {
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    let mut labels = Vec::new();
    let mut collection_key = None;
    let (collection, tags): (Option<&str>, &[&str]) = match segments.as_slice() {
        ["api", "collections", c, rest @ ..] => (Some(*c), collection_tags(method, rest)),
        ["api", "files", c, _, _] if method == Method::GET => (Some(*c), &["file"]),
        _ => (None, &[]),
    };
    if let (Some(c), false) = (collection.and_then(|c| registry.get(c)), tags.is_empty()) {
        for tag in tags {
            labels.push(format!("{}:{tag}", c.name));
        }
        for tag in tags {
            labels.push(format!("*:{tag}"));
        }
        // like PocketBase, collection rules count per collection and endpoint
        collection_key = Some(format!("{}{}{}", c.id, segments.join("/"), tags.join("")));
    }
    labels.push(format!("{method} {path}"));
    labels.push(path.to_string());
    (labels, collection_key)
}

/// PocketBase's rule lookup: exact labels in order, then prefix rules (labels ending in `/`).
fn find_rule<'a>(
    rules: &'a RateLimits,
    labels: &[String],
    audiences: &[&str],
) -> Option<&'a RateLimitRule> {
    let allowed = |rule: &RateLimitRule| audiences.contains(&rule.audience.as_str());
    for label in labels {
        if let Some(rule) = rules.rules.iter().find(|r| r.label == *label && allowed(r)) {
            return Some(rule);
        }
        if let Some(rule) = rules
            .rules
            .iter()
            .filter(|r| r.label.ends_with('/'))
            .find(|r| format!("{label}/").starts_with(&r.label) && allowed(r))
        {
            return Some(rule);
        }
    }
    None
}

struct Window {
    started: Instant,
    remaining: u32,
    duration: u64,
}

/// Fixed-window counters, per limiter and client.
#[derive(Default)]
pub struct RateLimiter {
    windows: Mutex<HashMap<(String, IpAddr), Window>>,
}

impl RateLimiter {
    fn allow(&self, limiter: String, client: IpAddr, rule: &RateLimitRule) -> bool {
        let now = Instant::now();
        let mut windows = self.windows.lock().expect("rate limiter lock");
        let window = windows.entry((limiter, client)).or_insert(Window {
            started: now,
            remaining: rule.max_requests,
            duration: rule.duration,
        });
        if now.duration_since(window.started).as_secs() >= window.duration {
            window.started = now;
            window.remaining = rule.max_requests;
        }
        if window.remaining == 0 {
            return false;
        }
        window.remaining -= 1;
        true
    }

    /// Forgets all counters (when the rules change).
    pub fn reset(&self) {
        self.windows.lock().expect("rate limiter lock").clear();
    }

    /// Forgets clients whose window ended a while ago.
    pub fn prune(&self) {
        let now = Instant::now();
        self.windows
            .lock()
            .expect("rate limiter lock")
            .retain(|_, w| now.duration_since(w.started).as_secs() < w.duration + 1800);
    }
}

/// Counts a request in its window: in memory while this instance is the only live one,
/// in the database when instances share it, so the limits apply across them (in memory
/// again if the database can't be reached).
async fn allow(core: &Core, limiter: String, ip: IpAddr, rule: &RateLimitRule) -> bool {
    if core.realtime.other_instances() > 0 {
        match shared_allow(core, &limiter, ip, rule).await {
            Ok(allowed) => return allowed,
            Err(err) => {
                tracing::debug!(error = %format!("{err:#}"), "shared rate limit failed; counting here")
            }
        }
    }
    core.rate_limiter.allow(limiter, ip, rule)
}

async fn shared_allow(
    core: &Core,
    limiter: &str,
    ip: IpAddr,
    rule: &RateLimitRule,
) -> anyhow::Result<bool> {
    let client = core
        .pool
        .get()
        .await
        .map_err(crate::db::describe_pool_error)?;
    let key = format!("{limiter}|{ip}");
    let max = i32::try_from(rule.max_requests).unwrap_or(i32::MAX);
    let duration = rule.duration as f64;
    let remaining: i32 = client
        .query_one(
            "INSERT INTO _slonbase.rate_limits AS r (key, started, remaining) VALUES ($1, now(), $2::int4 - 1) \
             ON CONFLICT (key) DO UPDATE SET \
               started = CASE WHEN r.started <= now() - make_interval(secs => $3::float8) THEN now() ELSE r.started END, \
               remaining = CASE WHEN r.started <= now() - make_interval(secs => $3::float8) THEN $2::int4 - 1 \
                                ELSE r.remaining - 1 END \
             RETURNING remaining",
            &[
                &key as &(dyn tokio_postgres::types::ToSql + Sync),
                &max,
                &duration,
            ],
        )
        .await?
        .get(0);
    Ok(remaining >= 0)
}

fn too_many_requests() -> Response {
    ApiError::new(StatusCode::TOO_MANY_REQUESTS, "Too Many Requests.").into_response()
}

fn not_allowed() -> Response {
    ApiError::forbidden("Insufficient permissions to access the resource.").into_response()
}

/// Whether the request is for the `_superusers` collection (sign-in, OTP, records...).
fn targets_superusers(path: &str, registry: &Registry) -> bool {
    match path
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["api", "collections", c, ..] => registry
            .get(c)
            .is_some_and(|c| c.id == SUPERUSERS_COLLECTION_ID),
        _ => false,
    }
}

/// The middleware applying both guards.
pub async fn middleware(State(app): State<Arc<App>>, mut request: Request, next: Next) -> Response {
    let Ok(core) = app.core() else {
        return next.run(request).await;
    };
    let settings = core.settings();
    let remote = crate::ip::remote_ip(request.extensions());
    let ip = crate::ip::user_ip(request.headers(), remote, &settings.trusted_proxy);
    request.extensions_mut().insert(ClientIp(ip));

    let limits = &settings.rate_limits;
    if !limits.enabled && settings.superuser_ips.is_empty() {
        return next.run(request).await;
    }

    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(token_from_header)
        .map(str::to_string);
    let auth = match token {
        Some(token) => authenticate(&core, &token).await,
        None => RequestAuth::default(),
    };
    request.extensions_mut().insert(ResolvedAuth(auth.clone()));
    let registry = core.registry();
    let path = request.uri().path().to_string();

    if !superuser_ip_allowed(&settings.superuser_ips, ip)
        && (auth.is_superuser() || targets_superusers(&path, &registry))
    {
        return not_allowed();
    }

    let excluded = ip.is_some_and(|ip| crate::ip::in_any(&limits.excluded_ips, ip));
    if limits.enabled && !auth.is_superuser() && !excluded {
        if let Some(ip) = ip {
            let (labels, collection_key) = labels(request.method(), &path, &registry);
            let audiences: &[&str] = if auth.any_record().is_some() {
                &["", "@auth"]
            } else {
                &["", "@guest"]
            };
            if let Some(rule) = find_rule(limits, &labels, audiences) {
                let limiter = match collection_key {
                    Some(key) if rule.label.contains(':') => format!("{key}{}", rule.audience),
                    _ => format!("{}{}", rule.label, rule.audience),
                };
                if !allow(&core, limiter, ip, rule).await {
                    return too_many_requests();
                }
            }
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::model::Collection;
    use serde_json::json;

    fn registry() -> Registry {
        let c: Collection = serde_json::from_value(json!({
            "id": "pbc_posts", "name": "posts", "type": "base",
        }))
        .unwrap();
        Registry::new(vec![c])
    }

    fn rule(label: &str, audience: &str) -> RateLimitRule {
        RateLimitRule {
            label: label.into(),
            audience: audience.into(),
            duration: 10,
            max_requests: 2,
        }
    }

    #[test]
    fn labels_and_rules() {
        let (create_labels, key) =
            labels(&Method::POST, "/api/collections/posts/records", &registry());
        assert_eq!(
            create_labels,
            vec![
                "posts:create",
                "*:create",
                "POST /api/collections/posts/records",
                "/api/collections/posts/records"
            ]
        );
        assert!(key.is_some());
        let (auth_labels, _) = labels_for_auth();
        assert_eq!(
            &auth_labels[..4],
            &[
                "posts:authWithPassword",
                "posts:auth",
                "*:authWithPassword",
                "*:auth"
            ]
        );

        let limits = RateLimits {
            rules: vec![
                rule("/api/", ""),
                rule("*:create", ""),
                rule("posts:create", "@auth"),
            ],
            excluded_ips: vec![],
            enabled: true,
        };
        let (create, _) = labels(&Method::POST, "/api/collections/posts/records", &registry());
        assert_eq!(
            find_rule(&limits, &create, &["", "@guest"]).unwrap().label,
            "*:create"
        );
        assert_eq!(
            find_rule(&limits, &create, &["", "@auth"]).unwrap().label,
            "posts:create"
        );
        let (list, _) = labels(&Method::GET, "/api/collections/posts/records", &registry());
        assert_eq!(
            find_rule(&limits, &list, &["", "@guest"]).unwrap().label,
            "/api/"
        );
        let (health, _) = labels(&Method::GET, "/api/health", &registry());
        assert_eq!(find_rule(&limits, &health, &[""]).unwrap().label, "/api/");
        let (other, _) = labels(&Method::GET, "/_/", &registry());
        assert!(find_rule(&limits, &other, &[""]).is_none());

        let method_rule = RateLimits {
            rules: vec![rule("POST /api/files/token", "")],
            ..limits
        };
        let (token, _) = labels(&Method::POST, "/api/files/token", &registry());
        assert!(find_rule(&method_rule, &token, &[""]).is_some());
        let (token_get, _) = labels(&Method::GET, "/api/files/token", &registry());
        assert!(find_rule(&method_rule, &token_get, &[""]).is_none());
    }

    fn labels_for_auth() -> (Vec<String>, Option<String>) {
        labels(
            &Method::POST,
            "/api/collections/posts/auth-with-password",
            &registry(),
        )
    }

    #[test]
    fn fixed_windows() {
        let limiter = RateLimiter::default();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let other: IpAddr = "10.0.0.2".parse().unwrap();
        let r = rule("/api/", "");
        assert!(limiter.allow("a".into(), ip, &r));
        assert!(limiter.allow("a".into(), ip, &r));
        assert!(!limiter.allow("a".into(), ip, &r));
        assert!(limiter.allow("a".into(), other, &r), "per client");
        assert!(limiter.allow("b".into(), ip, &r), "per limiter");
        limiter.reset();
        assert!(limiter.allow("a".into(), ip, &r));
    }

    #[test]
    fn superuser_ips() {
        assert!(superuser_ip_allowed(&[], None));
        let list = vec!["10.0.0.0/8".to_string()];
        assert!(superuser_ip_allowed(
            &list,
            Some("10.2.3.4".parse().unwrap())
        ));
        assert!(!superuser_ip_allowed(
            &list,
            Some("8.8.8.8".parse().unwrap())
        ));
        assert!(!superuser_ip_allowed(&list, None));
        assert!(targets_superusers(
            "/api/collections/_superusers/auth-with-password",
            &{
                let c: Collection = serde_json::from_value(json!({
                    "id": SUPERUSERS_COLLECTION_ID, "name": "_superusers", "type": "auth",
                }))
                .unwrap();
                Registry::new(vec![c])
            }
        ));
    }
}

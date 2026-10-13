//! OAuth2 providers: presets for common providers and any OpenID Connect provider,
//! sign-in URLs (with PKCE), the code exchange and fetching the provider's user.
//!
//! Only emails the provider reports as verified are used, since an email links the
//! provider's account to an existing record with that email.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::collections::model::OAuth2Provider;

const TIMEOUT: Duration = Duration::from_secs(15);
/// Largest token or user info response accepted.
const MAX_RESPONSE: usize = 1 << 20;
/// Largest avatar downloaded on sign-up.
pub const MAX_AVATAR: usize = 5 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Google,
    Github,
    Gitlab,
    Discord,
    Microsoft,
    Facebook,
    Oidc,
}

struct Preset {
    name: &'static str,
    kind: Kind,
    display_name: &'static str,
    auth_url: &'static str,
    token_url: &'static str,
    user_info_url: &'static str,
    scopes: &'static [&'static str],
}

const PRESETS: &[Preset] = &[
    Preset {
        name: "google",
        kind: Kind::Google,
        display_name: "Google",
        auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
        token_url: "https://oauth2.googleapis.com/token",
        user_info_url: "https://www.googleapis.com/oauth2/v3/userinfo",
        scopes: &[
            "https://www.googleapis.com/auth/userinfo.profile",
            "https://www.googleapis.com/auth/userinfo.email",
        ],
    },
    Preset {
        name: "github",
        kind: Kind::Github,
        display_name: "GitHub",
        auth_url: "https://github.com/login/oauth/authorize",
        token_url: "https://github.com/login/oauth/access_token",
        user_info_url: "https://api.github.com/user",
        scopes: &["read:user", "user:email"],
    },
    Preset {
        name: "gitlab",
        kind: Kind::Gitlab,
        display_name: "GitLab",
        auth_url: "https://gitlab.com/oauth/authorize",
        token_url: "https://gitlab.com/oauth/token",
        user_info_url: "https://gitlab.com/api/v4/user",
        scopes: &["read_user"],
    },
    Preset {
        name: "discord",
        kind: Kind::Discord,
        display_name: "Discord",
        auth_url: "https://discord.com/api/oauth2/authorize",
        token_url: "https://discord.com/api/oauth2/token",
        user_info_url: "https://discord.com/api/users/@me",
        scopes: &["identify", "email"],
    },
    Preset {
        name: "microsoft",
        kind: Kind::Microsoft,
        display_name: "Microsoft",
        auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
        token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        user_info_url: "https://graph.microsoft.com/v1.0/me",
        scopes: &["User.Read"],
    },
    Preset {
        name: "facebook",
        kind: Kind::Facebook,
        display_name: "Facebook",
        auth_url: "https://www.facebook.com/v18.0/dialog/oauth",
        token_url: "https://graph.facebook.com/v18.0/oauth/access_token",
        user_info_url: "https://graph.facebook.com/me?fields=name,email,picture.type(large)",
        scopes: &["email"],
    },
    Preset {
        name: "oidc",
        kind: Kind::Oidc,
        display_name: "OIDC",
        auth_url: "",
        token_url: "",
        user_info_url: "",
        scopes: &["openid", "email", "profile"],
    },
    Preset {
        name: "oidc2",
        kind: Kind::Oidc,
        display_name: "OIDC",
        auth_url: "",
        token_url: "",
        user_info_url: "",
        scopes: &["openid", "email", "profile"],
    },
    Preset {
        name: "oidc3",
        kind: Kind::Oidc,
        display_name: "OIDC",
        auth_url: "",
        token_url: "",
        user_info_url: "",
        scopes: &["openid", "email", "profile"],
    },
];

fn random_text(len: usize) -> String {
    crate::ids::generate_from_pattern(&format!("[a-zA-Z0-9]{{{len}}}")).expect("valid pattern")
}

fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// Whether `name` is a supported provider.
pub fn is_known(name: &str) -> bool {
    preset(name).is_some()
}

/// Whether the provider needs its URLs configured (OpenID Connect).
pub fn needs_urls(name: &str) -> bool {
    preset(name).is_some_and(|p| p.kind == Kind::Oidc)
}

/// A configured provider with the preset's defaults filled in.
pub struct Provider {
    pub name: String,
    pub display_name: String,
    client_id: String,
    client_secret: String,
    auth_url: String,
    token_url: String,
    user_info_url: String,
    scopes: Vec<String>,
    pub pkce: bool,
    kind: Kind,
}

impl Provider {
    pub fn new(config: &OAuth2Provider) -> Option<Self> {
        let preset = preset(&config.name)?;
        let or = |own: &str, default: &str| {
            if own.is_empty() {
                default.to_string()
            } else {
                own.to_string()
            }
        };
        Some(Self {
            name: config.name.clone(),
            display_name: or(&config.display_name, preset.display_name),
            client_id: config.client_id.clone(),
            client_secret: config.client_secret.clone(),
            auth_url: or(&config.auth_url, preset.auth_url),
            token_url: or(&config.token_url, preset.token_url),
            user_info_url: or(&config.user_info_url, preset.user_info_url),
            scopes: preset.scopes.iter().map(|s| s.to_string()).collect(),
            pkce: config.pkce.unwrap_or(true),
            kind: preset.kind,
        })
    }

    /// The `auth-methods` entry: a sign-in URL ending in `redirect_uri=` (clients append
    /// their redirect URL), with a fresh state and PKCE verifier.
    pub fn auth_method(&self) -> Value {
        let state = random_text(30);
        let mut query = form_urlencoded::Serializer::new(String::new());
        query.append_pair("client_id", &self.client_id);
        let mut out = Map::new();
        out.insert("name".into(), self.name.clone().into());
        out.insert("displayName".into(), self.display_name.clone().into());
        out.insert("state".into(), state.clone().into());
        if self.pkce {
            let verifier = random_text(43);
            let challenge = B64.encode(Sha256::digest(verifier.as_bytes()));
            query.append_pair("code_challenge", &challenge);
            query.append_pair("code_challenge_method", "S256");
            out.insert("codeVerifier".into(), verifier.into());
            out.insert("codeChallenge".into(), challenge.into());
            out.insert("codeChallengeMethod".into(), "S256".into());
        } else {
            out.insert("codeVerifier".into(), "".into());
            out.insert("codeChallenge".into(), "".into());
            out.insert("codeChallengeMethod".into(), "".into());
        }
        query.append_pair("response_type", "code");
        query.append_pair("scope", &self.scopes.join(" "));
        query.append_pair("state", &state);
        let separator = if self.auth_url.contains('?') {
            '&'
        } else {
            '?'
        };
        let url = format!(
            "{}{separator}{}&redirect_uri=",
            self.auth_url,
            query.finish()
        );
        out.insert("authURL".into(), url.clone().into());
        // older SDKs read `authUrl`
        out.insert("authUrl".into(), url.into());
        Value::Object(out)
    }

    /// Exchanges an authorization code for tokens.
    pub async fn exchange(&self, code: &str, verifier: &str, redirect_url: &str) -> Result<Token> {
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_url),
            ("client_id", &self.client_id),
            ("client_secret", &self.client_secret),
        ];
        if !verifier.is_empty() {
            form.push(("code_verifier", verifier));
        }
        let response = client()
            .post(&self.token_url)
            .timeout(TIMEOUT)
            .header("accept", "application/json")
            .form(&form)
            .send()
            .await
            .context("token request failed")?;
        let status = response.status();
        let body = read_limited(response, MAX_RESPONSE).await?;
        let data: Map<String, Value> = match serde_json::from_slice(&body) {
            Ok(Value::Object(map)) => map,
            // some providers answer form-encoded
            _ => form_urlencoded::parse(&body)
                .map(|(k, v)| (k.into_owned(), Value::String(v.into_owned())))
                .collect(),
        };
        let access_token = data
            .get("access_token")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !status.is_success() || access_token.is_empty() {
            let error = data
                .get("error_description")
                .or_else(|| data.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("no access token");
            bail!("the token endpoint answered {status}: {error}");
        }
        let expires_in = match data.get("expires_in") {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) => s.parse().ok(),
            _ => None,
        };
        Ok(Token {
            access_token: access_token.to_string(),
            refresh_token: data
                .get("refresh_token")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            expiry: expires_in
                .filter(|s| *s > 0)
                .map(|s| {
                    crate::datetime::format(&(chrono::Utc::now() + chrono::Duration::seconds(s)))
                })
                .unwrap_or_default(),
        })
    }

    /// Fetches the provider's user with an access token.
    pub async fn user(&self, token: &Token) -> Result<AuthUser> {
        let raw = get_json(&self.user_info_url, &token.access_token).await?;
        let mut user = extract(self.kind, &raw);
        if self.kind == Kind::Github && user.email.is_empty() {
            // a private email: ask for the verified primary one (optional)
            let emails_url = format!("{}/emails", self.user_info_url.trim_end_matches('/'));
            if let Ok(Value::Array(emails)) = get_json(&emails_url, &token.access_token).await {
                user.email = emails
                    .iter()
                    .find(|e| e["primary"] == true && e["verified"] == true)
                    .and_then(|e| e["email"].as_str())
                    .unwrap_or_default()
                    .to_string();
            }
        }
        if user.id.is_empty() {
            bail!("the provider's user has no id");
        }
        user.access_token = token.access_token.clone();
        user.refresh_token = token.refresh_token.clone();
        user.expiry = token.expiry.clone();
        user.raw = raw;
        Ok(user)
    }
}

pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    pub expiry: String,
}

#[derive(Debug, Default)]
pub struct AuthUser {
    pub id: String,
    pub name: String,
    pub username: String,
    /// Empty unless the provider verified it.
    pub email: String,
    pub avatar_url: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expiry: String,
    pub raw: Value,
}

impl AuthUser {
    pub fn meta(&self, is_new: bool) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "username": self.username,
            "email": self.email,
            "isNew": is_new,
            "avatarURL": self.avatar_url,
            "rawUser": self.raw,
            "accessToken": self.access_token,
            "refreshToken": self.refresh_token,
            "expiry": self.expiry,
        })
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn is_true(v: &Value) -> bool {
    v == &Value::Bool(true) || v.as_str() == Some("true")
}

fn extract(kind: Kind, raw: &Value) -> AuthUser {
    let mut user = AuthUser::default();
    match kind {
        Kind::Google | Kind::Oidc => {
            user.id = text(&raw["sub"]);
            user.name = text(&raw["name"]);
            user.username = text(&raw["preferred_username"]);
            user.avatar_url = text(&raw["picture"]);
            if is_true(&raw["email_verified"]) {
                user.email = text(&raw["email"]);
            }
        }
        Kind::Github | Kind::Gitlab => {
            user.id = text(&raw["id"]);
            user.name = text(&raw["name"]);
            user.username = text(
                &raw[if kind == Kind::Github {
                    "login"
                } else {
                    "username"
                }],
            );
            user.avatar_url = text(&raw["avatar_url"]);
            // both only return confirmed emails here
            user.email = text(&raw["email"]);
        }
        Kind::Discord => {
            user.id = text(&raw["id"]);
            user.username = text(&raw["username"]);
            user.name = match text(&raw["global_name"]) {
                name if name.is_empty() => user.username.clone(),
                name => name,
            };
            let avatar = text(&raw["avatar"]);
            if !avatar.is_empty() {
                user.avatar_url = format!(
                    "https://cdn.discordapp.com/avatars/{}/{avatar}.png",
                    user.id
                );
            }
            if is_true(&raw["verified"]) {
                user.email = text(&raw["email"]);
            }
        }
        Kind::Microsoft => {
            user.id = text(&raw["id"]);
            user.name = text(&raw["displayName"]);
            // Microsoft doesn't say whether `mail` is verified, so it isn't used
        }
        Kind::Facebook => {
            user.id = text(&raw["id"]);
            user.name = text(&raw["name"]);
            user.avatar_url = text(&raw["picture"]["data"]["url"]);
            // Facebook only returns confirmed emails
            user.email = text(&raw["email"]);
        }
    }
    user.email = user.email.trim().to_string();
    user
}

fn client() -> &'static reqwest::Client {
    crate::http::client()
}

async fn read_limited(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > limit {
            bail!("the response is too large");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn get_json(url: &str, access_token: &str) -> Result<Value> {
    let response = client()
        .get(url)
        .timeout(TIMEOUT)
        .bearer_auth(access_token)
        .header("accept", "application/json")
        .send()
        .await
        .context("user info request failed")?;
    let status = response.status();
    let body = read_limited(response, MAX_RESPONSE).await?;
    if !status.is_success() {
        bail!("the user info endpoint answered {status}");
    }
    serde_json::from_slice(&body).context("invalid user info response")
}

/// Downloads a file (an avatar), or `None` if it fails or is too large.
pub async fn download(url: &str) -> Option<Vec<u8>> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let response = client().get(url).timeout(TIMEOUT).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    read_limited(response, MAX_AVATAR).await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_url_has_pkce_and_ends_with_redirect() {
        let provider = Provider::new(&OAuth2Provider {
            name: "github".into(),
            client_id: "abc".into(),
            client_secret: "secret".into(),
            ..Default::default()
        })
        .unwrap();
        let method = provider.auth_method();
        let url = method["authURL"].as_str().unwrap();
        assert!(url.starts_with("https://github.com/login/oauth/authorize?client_id=abc&"));
        assert!(url.ends_with("&redirect_uri="));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("state={}", method["state"].as_str().unwrap())));
        assert!(!url.contains("secret"));
        let verifier = method["codeVerifier"].as_str().unwrap();
        assert_eq!(
            method["codeChallenge"].as_str().unwrap(),
            B64.encode(Sha256::digest(verifier.as_bytes()))
        );
        assert_eq!(method["displayName"], "GitHub");
    }

    #[test]
    fn only_verified_emails() {
        let user = extract(
            Kind::Oidc,
            &json!({ "sub": "1", "email": "a@x.com", "email_verified": false }),
        );
        assert_eq!(user.email, "");
        let user = extract(
            Kind::Google,
            &json!({ "sub": "1", "email": "a@x.com", "email_verified": true }),
        );
        assert_eq!(user.email, "a@x.com");
        let user = extract(
            Kind::Discord,
            &json!({ "id": "7", "username": "u", "email": "a@x.com", "verified": false, "avatar": "h" }),
        );
        assert_eq!(user.email, "");
        assert_eq!(user.name, "u");
        assert_eq!(
            user.avatar_url,
            "https://cdn.discordapp.com/avatars/7/h.png"
        );
        let user = extract(Kind::Microsoft, &json!({ "id": "m", "mail": "a@x.com" }));
        assert_eq!(user.email, "");
        let user = extract(Kind::Github, &json!({ "id": 42, "login": "octo" }));
        assert_eq!((user.id.as_str(), user.username.as_str()), ("42", "octo"));
    }
}

//! OAuth2 providers: presets for common providers and any OpenID Connect provider,
//! sign-in URLs (with PKCE), the code exchange and fetching the provider's user.
//!
//! Only emails the provider reports as verified are used, since an email links the
//! provider's account to an existing record with that email.
//!
//! An OpenID Connect provider without a user info URL is read from the `id_token` of the
//! token response, validated as PocketBase does: its audience and times, and with the
//! provider's `issuers` and `jwksURL` extra options, its issuer and signature.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::collections::model::OAuth2Provider;

const TIMEOUT: Duration = Duration::from_secs(15);
/// Apple's signing keys, which its `id_token`s must be signed with.
const APPLE_JWKS: &str = "https://appleid.apple.com/auth/keys";
const APPLE_ISSUER: &str = "https://appleid.apple.com";
/// Clock skew allowed when checking an `id_token`'s times.
const ID_TOKEN_LEEWAY: i64 = 5 * 60;
/// Largest token or user info response accepted.
const MAX_RESPONSE: usize = 1 << 20;
/// Largest avatar downloaded on sign-up.
pub const MAX_AVATAR: usize = 5 << 20;
/// Redirects followed when downloading an avatar.
const MAX_REDIRECTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Google,
    Github,
    Gitlab,
    Discord,
    Microsoft,
    Facebook,
    Apple,
    Bitbucket,
    Twitch,
    Spotify,
    Patreon,
    Kakao,
    Twitter,
    Oidc,
}

impl Kind {
    /// Whether the token endpoint wants the client's credentials with HTTP Basic
    /// authentication instead of in the form.
    fn basic_auth(self) -> bool {
        matches!(self, Kind::Bitbucket | Kind::Twitter)
    }
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
    // no user info endpoint: the user comes from the id_token (see [`APPLE_JWKS`])
    Preset {
        name: "apple",
        kind: Kind::Apple,
        display_name: "Apple",
        auth_url: "https://appleid.apple.com/auth/authorize",
        token_url: "https://appleid.apple.com/auth/token",
        user_info_url: "",
        scopes: &["name", "email"],
    },
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
        name: "bitbucket",
        kind: Kind::Bitbucket,
        display_name: "Bitbucket",
        auth_url: "https://bitbucket.org/site/oauth2/authorize",
        token_url: "https://bitbucket.org/site/oauth2/access_token",
        user_info_url: "https://api.bitbucket.org/2.0/user",
        scopes: &["account", "email"],
    },
    Preset {
        name: "twitch",
        kind: Kind::Twitch,
        display_name: "Twitch",
        auth_url: "https://id.twitch.tv/oauth2/authorize",
        token_url: "https://id.twitch.tv/oauth2/token",
        user_info_url: "https://api.twitch.tv/helix/users",
        scopes: &["user:read:email"],
    },
    Preset {
        name: "spotify",
        kind: Kind::Spotify,
        display_name: "Spotify",
        auth_url: "https://accounts.spotify.com/authorize",
        token_url: "https://accounts.spotify.com/api/token",
        user_info_url: "https://api.spotify.com/v1/me",
        scopes: &["user-read-private", "user-read-email"],
    },
    Preset {
        name: "patreon",
        kind: Kind::Patreon,
        display_name: "Patreon",
        auth_url: "https://www.patreon.com/oauth2/authorize",
        token_url: "https://www.patreon.com/api/oauth2/token",
        user_info_url: "https://www.patreon.com/api/oauth2/v2/identity?fields%5Buser%5D=full_name,email,is_email_verified,vanity,image_url",
        scopes: &["identity", "identity[email]"],
    },
    Preset {
        name: "kakao",
        kind: Kind::Kakao,
        display_name: "Kakao",
        auth_url: "https://kauth.kakao.com/oauth/authorize",
        token_url: "https://kauth.kakao.com/oauth/token",
        user_info_url: "https://kapi.kakao.com/v2/user/me",
        scopes: &["account_email", "profile_nickname", "profile_image"],
    },
    // X requires PKCE (on by default)
    Preset {
        name: "twitter",
        kind: Kind::Twitter,
        display_name: "X",
        auth_url: "https://twitter.com/i/oauth2/authorize",
        token_url: "https://api.twitter.com/2/oauth2/token",
        user_info_url: "https://api.twitter.com/2/users/me?user.fields=id,name,username,profile_image_url",
        scopes: &["users.read", "tweet.read"],
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
    extra: Map<String, Value>,
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
            extra: config.extra.clone(),
        })
    }

    /// Whether this is Apple, which sends the user's name only with the first redirect.
    pub fn is_apple(&self) -> bool {
        self.kind == Kind::Apple
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
        if self.kind == Kind::Apple {
            // Apple POSTs the redirect, with the user's name the first time
            query.append_pair("response_mode", "form_post");
        }
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
        ];
        if !self.kind.basic_auth() {
            form.push(("client_id", &self.client_id));
            form.push(("client_secret", &self.client_secret));
        }
        if !verifier.is_empty() {
            form.push(("code_verifier", verifier));
        }
        let mut request = client()
            .post(&self.token_url)
            .timeout(TIMEOUT)
            .header("accept", "application/json");
        if self.kind.basic_auth() {
            request = request.basic_auth(&self.client_id, Some(&self.client_secret));
        }
        let response = request
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
            id_token: data
                .get("id_token")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
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

    /// Downloads the user's avatar (see [`download`]). The provider's own hosts are
    /// trusted, since a superuser configured them (a self-hosted provider may be on a
    /// private network).
    pub async fn download_avatar(&self, url: &str) -> Option<Vec<u8>> {
        let own_hosts: Vec<String> = [&self.auth_url, &self.token_url, &self.user_info_url]
            .into_iter()
            .filter_map(|u| {
                Some(
                    reqwest::Url::parse(u)
                        .ok()?
                        .host_str()?
                        .to_ascii_lowercase(),
                )
            })
            .collect();
        download(url, &own_hosts).await
    }

    /// Fetches the provider's user with an access token.
    pub async fn user(&self, token: &Token) -> Result<AuthUser> {
        let raw = if self.kind == Kind::Apple
            || (self.kind == Kind::Oidc && self.user_info_url.is_empty())
        {
            self.id_token_claims(&token.id_token, chrono::Utc::now().timestamp())
                .await
                .context("invalid id_token")?
        } else if self.kind == Kind::Twitch {
            // Twitch's API wants the client id along with the token
            get_json_with(
                &self.user_info_url,
                &token.access_token,
                &[("client-id", &self.client_id)],
            )
            .await?
        } else {
            get_json(&self.user_info_url, &token.access_token).await?
        };
        if self.kind == Kind::Bitbucket && raw["account_status"].as_str() != Some("active") {
            bail!("the Bitbucket account isn't active");
        }
        let mut user = extract(self.kind, &raw);
        if self.kind == Kind::Bitbucket {
            // the emails are separate: use the confirmed primary one (optional)
            let emails_url = format!("{}/emails", self.user_info_url.trim_end_matches('/'));
            if let Ok(emails) = get_json(&emails_url, &token.access_token).await {
                user.email = emails["values"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|e| e["is_primary"] == true && e["is_confirmed"] == true)
                    .and_then(|e| e["email"].as_str())
                    .unwrap_or_default()
                    .to_string();
            }
        }
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

    /// The claims of a valid `id_token` (`now` in Unix seconds): the audience must include the
    /// client id, the times must hold (within [`ID_TOKEN_LEEWAY`]), and with the `issuers` and
    /// `jwksURL` extra options, the issuer must be one of them and the signature valid.
    /// Apple's are always checked: `exp` is required, the issuer is Apple and the signature is
    /// verified with Apple's keys.
    async fn id_token_claims(&self, id_token: &str, now: i64) -> Result<Value> {
        let apple = self.kind == Kind::Apple;
        let jwt = Jwt::parse(id_token)?;
        let claims = &jwt.claims;
        let time = |name: &str| -> Result<Option<f64>> {
            match claims.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Number(n)) => Ok(n.as_f64()),
                Some(_) => bail!("{name} must be a number"),
            }
        };
        let now = now as f64;
        let leeway = ID_TOKEN_LEEWAY as f64;
        if apple && time("exp")?.is_none() {
            bail!("the token has no exp");
        }
        if time("exp")?.is_some_and(|exp| now - leeway >= exp) {
            bail!("the token is expired");
        }
        if time("nbf")?.is_some_and(|nbf| now + leeway < nbf) {
            bail!("the token isn't valid yet");
        }
        if time("iat")?.is_some_and(|iat| now + leeway < iat) {
            bail!("the token was issued in the future");
        }
        let audience = match claims.get("aud") {
            Some(Value::String(aud)) => vec![aud.as_str()],
            Some(Value::Array(auds)) => auds.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !audience
            .iter()
            .any(|aud| crate::ids::constant_time_eq(aud, &self.client_id))
        {
            bail!("the token's audience isn't the client id");
        }

        let issuers = if apple {
            vec![APPLE_ISSUER.to_string()]
        } else {
            string_list(self.extra.get("issuers"))
        };
        if !issuers.is_empty() {
            let issuer = claims
                .get("iss")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !issuers
                .iter()
                .any(|i| crate::ids::constant_time_eq(i, issuer))
            {
                bail!("the token's issuer {issuer:?} isn't one of {issuers:?}");
            }
        }

        // the token came straight from the provider over TLS, so checking its signature is
        // optional (as OpenID Connect allows), and done when the keys are configured
        let jwks_url = match self.extra.get("jwksURL").and_then(Value::as_str) {
            Some(url) if !url.is_empty() => url,
            _ if apple => APPLE_JWKS,
            _ => "",
        };
        if !jwks_url.is_empty() {
            let kid = jwt
                .header
                .get("kid")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if kid.is_empty() {
                bail!("the token has no kid header");
            }
            let keys = get_json(jwks_url, "").await.context("fetching the JWKS")?;
            let key = keys["keys"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|k| k["kid"] == kid && k["alg"].as_str().is_some_and(|a| !a.is_empty()))
                .with_context(|| format!("no key with kid {kid:?} and an alg in the JWKS"))?;
            jwt.verify(key)?;
        }
        Ok(Value::Object(jwt.claims))
    }
}

/// Apple's client secret: an ES256 JWT signed with the "Sign in with Apple" private key (a
/// PKCS#8 PEM, as Apple's `.p8` files are), valid for `duration` seconds from `now`.
pub fn apple_client_secret(
    client_id: &str,
    team_id: &str,
    key_id: &str,
    private_key: &str,
    duration: i64,
    now: i64,
) -> Result<String> {
    use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
    let pem = private_key.trim();
    let body = pem
        .split_once("-----BEGIN PRIVATE KEY-----")
        .and_then(|(_, rest)| rest.split_once("-----END PRIVATE KEY-----"))
        .map(|(body, _)| body)
        .context("the key isn't a PEM private key")?;
    let der = base64::engine::general_purpose::STANDARD
        .decode(body.split_whitespace().collect::<String>())
        .context("the key isn't valid base64")?;
    let rng = ring::rand::SystemRandom::new();
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng)
        .map_err(|e| anyhow::anyhow!("the key isn't a P-256 private key: {e}"))?;
    let header = json!({ "alg": "ES256", "kid": key_id, "typ": "JWT" });
    let claims = json!({
        "iss": team_id,
        "sub": client_id,
        "aud": [APPLE_ISSUER],
        "exp": now + duration,
        "iat": now,
    });
    let signed = format!(
        "{}.{}",
        B64.encode(header.to_string()),
        B64.encode(claims.to_string())
    );
    let signature = key
        .sign(&rng, signed.as_bytes())
        .map_err(|_| anyhow::anyhow!("signing failed"))?;
    Ok(format!("{signed}.{}", B64.encode(signature.as_ref())))
}

/// A string list option: a JSON array, or a string of space-separated values.
fn string_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| match i {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        Some(Value::String(s)) => s.split_whitespace().map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// A decoded (not yet verified) JSON Web Token.
struct Jwt {
    header: Map<String, Value>,
    claims: Map<String, Value>,
    /// The signed part, `header.payload`.
    signed: String,
    signature: Vec<u8>,
}

impl Jwt {
    fn parse(token: &str) -> Result<Self> {
        let decode = |part: &str| B64.decode(part.trim_end_matches('='));
        let [header, payload, signature] = token.split('.').collect::<Vec<_>>()[..] else {
            bail!("the token isn't a JWT");
        };
        let object = |part: &str, what: &str| -> Result<Map<String, Value>> {
            match serde_json::from_slice(&decode(part).with_context(|| format!("bad {what}"))?) {
                Ok(Value::Object(map)) => Ok(map),
                _ => bail!("the token's {what} isn't a JSON object"),
            }
        };
        Ok(Self {
            header: object(header, "header")?,
            claims: object(payload, "claims")?,
            signed: format!("{header}.{payload}"),
            signature: decode(signature).context("bad signature encoding")?,
        })
    }

    /// Checks the signature with a JWK: RSA (`RS*`, `PS*`) or Ed25519 (`EdDSA`). The
    /// token's `alg` must be the key's.
    fn verify(&self, key: &Value) -> Result<()> {
        use ring::signature;
        let alg = key["alg"].as_str().unwrap_or_default();
        if self.header.get("alg").and_then(Value::as_str) != Some(alg) {
            bail!("the token's alg isn't the key's ({alg})");
        }
        let part = |name: &str| -> Result<Vec<u8>> {
            let text = key[name].as_str().unwrap_or_default();
            B64.decode(text.trim_end_matches('='))
                .with_context(|| format!("bad key {name}"))
        };
        let (message, sig) = (self.signed.as_bytes(), self.signature.as_slice());
        let valid = match key["kty"].as_str().unwrap_or_default() {
            "RSA" => {
                let params: &signature::RsaParameters = match alg {
                    "RS256" => &signature::RSA_PKCS1_2048_8192_SHA256,
                    "RS384" => &signature::RSA_PKCS1_2048_8192_SHA384,
                    "RS512" => &signature::RSA_PKCS1_2048_8192_SHA512,
                    "PS256" => &signature::RSA_PSS_2048_8192_SHA256,
                    "PS384" => &signature::RSA_PSS_2048_8192_SHA384,
                    "PS512" => &signature::RSA_PSS_2048_8192_SHA512,
                    _ => bail!("unsupported alg {alg} for an RSA key"),
                };
                let (n, e) = (part("n")?, part("e")?);
                signature::RsaPublicKeyComponents { n: &n, e: &e }
                    .verify(params, message, sig)
                    .is_ok()
            }
            "OKP" => {
                if key["crv"] != "Ed25519" || alg != "EdDSA" {
                    bail!("unsupported OKP key (must be Ed25519 with EdDSA)");
                }
                let x = part("x")?;
                signature::UnparsedPublicKey::new(&signature::ED25519, &x)
                    .verify(message, sig)
                    .is_ok()
            }
            other => bail!("unsupported key type {other:?} (must be RSA or OKP)"),
        };
        if !valid {
            bail!("the token's signature is invalid");
        }
        Ok(())
    }
}

pub struct Token {
    /// The OpenID Connect `id_token`, if the provider sent one.
    pub id_token: String,
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
    /// The user as PocketBase's hooks see it (`e.oauth2User`).
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "username": self.username,
            "email": self.email,
            "avatarURL": self.avatar_url,
            "rawUser": self.raw,
            "accessToken": self.access_token,
            "refreshToken": self.refresh_token,
            "expiry": self.expiry,
        })
    }

    pub fn from_json(v: &Value) -> Self {
        let s = |key: &str| text(&v[key]);
        Self {
            id: s("id"),
            name: s("name"),
            username: s("username"),
            email: s("email"),
            avatar_url: s("avatarURL"),
            access_token: s("accessToken"),
            refresh_token: s("refreshToken"),
            expiry: s("expiry"),
            raw: v["rawUser"].clone(),
        }
    }

    /// The `meta` of the sign-in response.
    pub fn meta(&self, is_new: bool) -> Value {
        let mut meta = self.to_json();
        meta["isNew"] = Value::Bool(is_new);
        meta
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
        Kind::Apple => {
            user.id = text(&raw["sub"]);
            // Apple sends the name only with the first redirect (see oauth2_api)
            user.name = text(&raw["name"]);
            if is_true(&raw["email_verified"]) {
                user.email = text(&raw["email"]);
            }
        }
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
        Kind::Bitbucket => {
            // the email comes from a second request (see `Provider::user`)
            user.id = text(&raw["uuid"]);
            user.username = text(&raw["username"]);
            user.name = text(&raw["display_name"]);
            user.avatar_url = text(&raw["links"]["avatar"]["href"]);
        }
        Kind::Twitch => {
            let data = &raw["data"][0];
            user.id = text(&data["id"]);
            user.username = text(&data["login"]);
            user.name = text(&data["display_name"]);
            user.avatar_url = text(&data["profile_image_url"]);
            // Twitch only returns verified emails
            user.email = text(&data["email"]);
        }
        Kind::Spotify => {
            user.id = text(&raw["id"]);
            user.name = text(&raw["display_name"]);
            user.avatar_url = text(&raw["images"][0]["url"]);
            // Spotify doesn't verify emails, so `email` isn't used
        }
        Kind::Patreon => {
            let data = &raw["data"];
            let attributes = &data["attributes"];
            user.id = text(&data["id"]);
            user.username = text(&attributes["vanity"]);
            user.name = text(&attributes["full_name"]);
            user.avatar_url = text(&attributes["image_url"]);
            if is_true(&attributes["is_email_verified"]) {
                user.email = text(&attributes["email"]);
            }
        }
        Kind::Kakao => {
            let account = &raw["kakao_account"];
            user.id = text(&raw["id"]);
            user.name = text(&account["profile"]["nickname"]);
            user.avatar_url = text(&account["profile"]["profile_image_url"]);
            if is_true(&account["is_email_valid"]) && is_true(&account["is_email_verified"]) {
                user.email = text(&account["email"]);
            }
        }
        Kind::Twitter => {
            // X doesn't give emails with these scopes
            let data = &raw["data"];
            user.id = text(&data["id"]);
            user.username = text(&data["username"]);
            user.name = text(&data["name"]);
            user.avatar_url = text(&data["profile_image_url"]);
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

/// GETs JSON, with the access token if there's one (none for the JWKS).
async fn get_json(url: &str, access_token: &str) -> Result<Value> {
    get_json_with(url, access_token, &[]).await
}

/// [`get_json`] with more headers.
async fn get_json_with(url: &str, access_token: &str, headers: &[(&str, &str)]) -> Result<Value> {
    let mut request = client()
        .get(url)
        .timeout(TIMEOUT)
        .header("accept", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    if !access_token.is_empty() {
        request = request.bearer_auth(access_token);
    }
    let response = request.send().await.context("request failed")?;
    let status = response.status();
    let body = read_limited(response, MAX_RESPONSE).await?;
    if !status.is_success() {
        bail!("{url} answered {status}");
    }
    serde_json::from_slice(&body).context("invalid JSON response")
}

/// Downloads a file (an avatar), or `None` if it fails or is too large. The URL comes from
/// the provider's user, which some providers let users set, so only public addresses are
/// fetched (not loopback, private networks or cloud metadata services), except on
/// `trusted_hosts`; redirects are checked the same way.
pub async fn download(url: &str, trusted_hosts: &[String]) -> Option<Vec<u8>> {
    let mut url = reqwest::Url::parse(url).ok()?;
    for _ in 0..=MAX_REDIRECTS {
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let host = url.host_str()?.to_ascii_lowercase();
        let builder = if trusted_hosts.contains(&host) {
            crate::http::builder()
        } else {
            let port = url.port_or_known_default()?;
            let addrs: Vec<std::net::SocketAddr> =
                tokio::net::lookup_host((host.trim_matches(['[', ']']), port))
                    .await
                    .ok()?
                    .collect();
            if addrs.is_empty() || !addrs.iter().all(|a| crate::ip::is_public(a.ip())) {
                return None;
            }
            // connect to the addresses just checked, so the name can't resolve elsewhere
            crate::http::builder().resolve_to_addrs(&host, &addrs)
        };
        let client = builder
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;
        let response = client.get(url.clone()).timeout(TIMEOUT).send().await.ok()?;
        if response.status().is_redirection() {
            let location = response.headers().get(reqwest::header::LOCATION)?;
            url = url.join(location.to_str().ok()?).ok()?;
            continue;
        }
        if !response.status().is_success() {
            return None;
        }
        return read_limited(response, MAX_AVATAR).await.ok();
    }
    None
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

    /// A server that answers every request with `body`.
    /// A throwaway 2048-bit RSA key (PKCS#1), for signing test tokens only.
    const TEST_RSA_KEY: &str = concat!(
        "MIIEpAIBAAKCAQEA0lb4DkAVsOUFmbhzORokgrGN0a/lgUBm82CTmMrcth4S+bPjSz5q7ns93OZMFvjYa9KKwwvy6e4YU3bR",
        "IGB8S1OZfZi1bXIhzyHJH5IRskLim2kaCaFGSJWys7GILfA24uc+pTv/NH00ZFemcUfs0uIaS/yUvdF+Bd+4v9SSGC0K7JRm",
        "5rubs2ZjoUoscbqZ7tdfGi949Od/Z9fDTQZUEcSViGlL71bUhDfKJli+gcgx+CrHjT9VnscvoNgVCuNxtFkTh5yR5bTbhgbX",
        "o0881LvQ/MW0f+oH1Y+E/tm2DwodDTAbaHbBk53+RpEIlBudatnUbfdPFZDe87JvXMl39QIDAQABAoIBAAXYLspOk4u3wWEV",
        "E/aJOGx9PICQD6N4w6ONkmxTbrCMw2A21GD52gTIXd6uc3zL60+LqRlkUpqXaAuhqIvqwmlhaElqHdhtrGzR9oGvEFjQrPU+",
        "hV+xIH/R0QZXL35/Rzckz8PNl/jAZd1H5jMNmZH+gFaOLR/dG1BUeBC0el67jLrCo+1LezpXmbHhnRlcjvYHEP8h+EHb1qkX",
        "hcgkwyI8dykn4a0WQ03+l8/OMl7jfGsAvFrceKzUTysq3ZptG1zf/TTqfEFRCX6ZWlf4+ZRJ8PTBPINZIrtwhRFTfQlubJ9u",
        "vRM//DwtPm9WSAOVHsrYyvjFtopZQ9R6Vl9rSkECgYEA++cETgNQjCXezqYPV0HEnq2uR6NM5BFjBRu9MqiDNRgclqH3PJ2M",
        "PlTtl+PxX2E221kcXqx5WrjgK4/jLc1Xuda/PVTmnUb0JzpEuSehdZD+AO58BXuLiiwwrySSVAkUSzM6COf6Fx9fcOvQqoBK",
        "hEztIcqjX5vpZk4dbj01OGUCgYEA1cLf/efcA+jKomjXKa5AcdLMuc8I59s1rcKWt31+lVh3BG+l0VPEoI02isUk2fvlXQT0",
        "6f0yAyZuiY1d80oZLGkmREmHQhvUaY6yYca0cuDWzUlyIjZTO6XSCpe0kG05JqR9PamHSCoXGmrRVB1/gf7qDV/kjOshnKwG",
        "eaLuIFECgYBPbevckYz/dB2KebBSqklOY6OVsTdYnubjQHjrFVmyDzVMhivIcEE9klgM0fJVSfkbN0EL1US6x5rgAZH36kzk",
        "K4PH2hKRu0LXb7clzwpJYPvG3Ww76lX4rrB3OYfueaJth+yfzE3+poTYvXLhoGxexUbspdHYHvh8qtr63NAARQKBgQDU3GrK",
        "31ETP4RROA9SfSfFcMHXIZhfscWBU21cKQadV2ulKjy39y+tJRG0HF73BsS+3Um5nB1qT6HxLanps9bfh5HInGOpbCcYOjXd",
        "8c4NO2ILKOETun8lvZksXSc8TteDaHciqSeCsA8zij0LAxjO7XZKfTwNK7Pb3N3R0HJKEQKBgQDo+r/7Q+ruFkQT2lQTO6Xr",
        "dG1U7ySAUjKWxYbhAy6dJq5Q7Z3rdLyH3ANp/TDoUfLGIC0qXrkDGUYbmObtz5XAjq1DhkRdVoZ4VZJ9yoSioBd2QZGp1owW",
        "/AObdAyrqwXSRxNkcdzw0WbqKBVN0JXOqyfELgbReR8t8J9Q0j+FrQ==",
    );

    fn b64(data: &[u8]) -> String {
        B64.encode(data)
    }

    /// A JWT with the claims, signed by `sign` (given `header.payload`).
    fn jwt(header: Value, claims: Value, sign: impl Fn(&[u8]) -> Vec<u8>) -> String {
        let signed = format!(
            "{}.{}",
            b64(header.to_string().as_bytes()),
            b64(claims.to_string().as_bytes())
        );
        let signature = sign(signed.as_bytes());
        format!("{signed}.{}", b64(&signature))
    }

    fn rsa_key() -> ring::signature::RsaKeyPair {
        let der = base64::engine::general_purpose::STANDARD
            .decode(TEST_RSA_KEY)
            .unwrap();
        ring::signature::RsaKeyPair::from_der(&der).unwrap()
    }

    fn rs256(key: &ring::signature::RsaKeyPair, message: &[u8]) -> Vec<u8> {
        let mut sig = vec![0; key.public().modulus_len()];
        key.sign(
            &ring::signature::RSA_PKCS1_SHA256,
            &ring::rand::SystemRandom::new(),
            message,
            &mut sig,
        )
        .unwrap();
        sig
    }

    /// Serves a JSON body at a local URL.
    async fn serve_json(body: String) -> String {
        let body: &'static str = Box::leak(body.into_boxed_str());
        serve(body).await.replace("/avatar.png", "/jwks")
    }

    fn oidc(extra: Value) -> Provider {
        Provider::new(&OAuth2Provider {
            name: "oidc".into(),
            client_id: "client-1".into(),
            auth_url: "https://id.example.com/auth".into(),
            token_url: "https://id.example.com/token".into(),
            extra: extra.as_object().unwrap().clone(),
            ..Default::default()
        })
        .unwrap()
    }

    const NOW: i64 = 1_800_000_000;

    #[tokio::test]
    async fn id_token_claims() {
        let unsigned = |claims: Value| jwt(json!({ "alg": "none" }), claims, |_| b"x".to_vec());
        let provider = oidc(json!({}));
        let good = json!({ "sub": "u1", "aud": "client-1", "exp": NOW + 60, "iat": NOW, "email": "a@x.com", "email_verified": true });
        let claims = provider
            .id_token_claims(&unsigned(good.clone()), NOW)
            .await
            .unwrap();
        assert_eq!(claims["sub"], "u1");
        let user = extract(Kind::Oidc, &claims);
        assert_eq!((user.id.as_str(), user.email.as_str()), ("u1", "a@x.com"));

        let with = |patch: Value| {
            let mut c = good.clone();
            for (k, v) in patch.as_object().unwrap() {
                c[k] = v.clone();
            }
            unsigned(c)
        };
        // fine: a list audience, times within the leeway, no exp at all
        for ok in [
            json!({ "aud": ["other", "client-1"] }),
            json!({ "exp": NOW - 200 }),
            json!({ "iat": NOW + 200, "nbf": NOW + 200 }),
            json!({ "exp": null }),
        ] {
            assert!(
                provider
                    .id_token_claims(&with(ok.clone()), NOW)
                    .await
                    .is_ok(),
                "{ok}"
            );
        }
        for bad in [
            json!({ "aud": "other" }),
            json!({ "aud": null }),
            json!({ "aud": ["other"] }),
            json!({ "exp": NOW - 301 }),
            json!({ "exp": "soon" }),
            json!({ "nbf": NOW + 301 }),
            json!({ "iat": NOW + 301 }),
        ] {
            assert!(
                provider
                    .id_token_claims(&with(bad.clone()), NOW)
                    .await
                    .is_err(),
                "{bad}"
            );
        }
        for bad in ["", "a.b", "a.b.c", "e30.e30"] {
            assert!(provider.id_token_claims(bad, NOW).await.is_err(), "{bad}");
        }

        // issuers, as a list or a space-separated string
        let token = with(json!({ "iss": "https://id.example.com" }));
        for issuers in [
            json!(["https://id.example.com"]),
            json!("https://other https://id.example.com"),
        ] {
            let provider = oidc(json!({ "issuers": issuers }));
            assert!(provider.id_token_claims(&token, NOW).await.is_ok());
        }
        let provider = oidc(json!({ "issuers": ["https://other"] }));
        assert!(provider.id_token_claims(&token, NOW).await.is_err());
        assert!(
            provider
                .id_token_claims(&unsigned(good), NOW)
                .await
                .is_err(),
            "no iss"
        );
    }

    #[tokio::test]
    async fn id_token_signatures() {
        use ring::signature::KeyPair;
        let rsa = rsa_key();
        let components: ring::rsa::PublicKeyComponents<Vec<u8>> = rsa.public().into();
        let ed = ring::signature::Ed25519KeyPair::from_pkcs8(
            ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
                .unwrap()
                .as_ref(),
        )
        .unwrap();
        let jwks = json!({ "keys": [
            { "kty": "RSA", "kid": "r1", "alg": "RS256", "use": "sig", "n": b64(&components.n), "e": b64(&components.e) },
            { "kty": "RSA", "kid": "noalg", "n": b64(&components.n), "e": b64(&components.e) },
            { "kty": "OKP", "kid": "e1", "alg": "EdDSA", "crv": "Ed25519", "x": b64(ed.public_key().as_ref()) },
        ] });
        let url = serve_json(jwks.to_string()).await;
        let provider = oidc(json!({ "jwksURL": url }));
        let claims = json!({ "sub": "u1", "aud": "client-1", "exp": NOW + 60 });

        let token = jwt(
            json!({ "alg": "RS256", "kid": "r1" }),
            claims.clone(),
            |m| rs256(&rsa, m),
        );
        assert!(provider.id_token_claims(&token, NOW).await.is_ok());
        let token = jwt(
            json!({ "alg": "EdDSA", "kid": "e1" }),
            claims.clone(),
            |m| ed.sign(m).as_ref().to_vec(),
        );
        assert!(provider.id_token_claims(&token, NOW).await.is_ok());

        // tampered claims, the wrong key, another alg, no kid, a key without alg
        let (head, rest) = token.split_once('.').unwrap();
        let (_, sig) = rest.split_once('.').unwrap();
        let forged = format!(
            "{head}.{}.{sig}",
            b64(json!({ "sub": "admin", "aud": "client-1" })
                .to_string()
                .as_bytes())
        );
        for (header, sign_rsa) in [
            (json!({ "alg": "RS256", "kid": "e1" }), true),
            (json!({ "alg": "RS512", "kid": "r1" }), true),
            (json!({ "alg": "RS256" }), true),
            (json!({ "alg": "RS256", "kid": "noalg" }), true),
            (json!({ "alg": "RS256", "kid": "r1" }), false),
        ] {
            let token = jwt(header.clone(), claims.clone(), |m| {
                if sign_rsa {
                    rs256(&rsa, m)
                } else {
                    ed.sign(m).as_ref().to_vec()
                }
            });
            assert!(
                provider.id_token_claims(&token, NOW).await.is_err(),
                "{header} {sign_rsa}"
            );
        }
        assert!(
            provider.id_token_claims(&forged, NOW).await.is_err(),
            "forged"
        );
    }

    async fn serve(body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        format!("http://{addr}/avatar.png")
    }

    #[tokio::test]
    async fn avatars_only_from_public_addresses() {
        let url = serve("secret").await;
        assert_eq!(download(&url, &[]).await, None, "loopback");
        assert_eq!(
            download(&url, &["127.0.0.1".into()]).await.as_deref(),
            Some(&b"secret"[..]),
            "the provider's own host"
        );
        let localhost = url.replace("127.0.0.1", "localhost");
        assert_eq!(download(&localhost, &[]).await, None, "a name for loopback");
        for url in [
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "file:///etc/passwd",
        ] {
            assert_eq!(download(url, &[]).await, None, "{url}");
        }
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

    #[test]
    fn extracts_the_newer_presets() {
        let user = extract(
            Kind::Bitbucket,
            &json!({ "uuid": "{b-1}", "username": "bb", "display_name": "B B",
                     "links": { "avatar": { "href": "https://a/b.png" } } }),
        );
        assert_eq!(
            (user.id.as_str(), user.username.as_str(), user.name.as_str()),
            ("{b-1}", "bb", "B B")
        );
        assert_eq!(user.avatar_url, "https://a/b.png");

        let user = extract(
            Kind::Twitch,
            &json!({ "data": [{ "id": "t1", "login": "tw", "display_name": "Tw",
                                "email": "t@x.com", "profile_image_url": "https://a/t.png" }] }),
        );
        assert_eq!(
            (
                user.id.as_str(),
                user.username.as_str(),
                user.email.as_str()
            ),
            ("t1", "tw", "t@x.com")
        );

        let user = extract(
            Kind::Spotify,
            &json!({ "id": "s1", "display_name": "Sp", "email": "s@x.com",
                     "images": [{ "url": "https://a/s.png" }] }),
        );
        assert_eq!((user.id.as_str(), user.email.as_str()), ("s1", ""));
        assert_eq!(user.avatar_url, "https://a/s.png");

        let patreon = |verified: bool| {
            json!({ "data": { "id": "p1", "attributes": {
                "full_name": "Pa", "vanity": "pa", "email": "p@x.com",
                "is_email_verified": verified, "image_url": "https://a/p.png" } } })
        };
        let user = extract(Kind::Patreon, &patreon(true));
        assert_eq!(
            (
                user.id.as_str(),
                user.username.as_str(),
                user.email.as_str()
            ),
            ("p1", "pa", "p@x.com")
        );
        assert_eq!(extract(Kind::Patreon, &patreon(false)).email, "");

        let kakao = |verified: bool| {
            json!({ "id": 123456, "kakao_account": {
                "profile": { "nickname": "Ka", "profile_image_url": "https://a/k.png" },
                "email": "k@x.com", "is_email_valid": true, "is_email_verified": verified } })
        };
        let user = extract(Kind::Kakao, &kakao(true));
        assert_eq!(
            (user.id.as_str(), user.name.as_str(), user.email.as_str()),
            ("123456", "Ka", "k@x.com")
        );
        assert_eq!(extract(Kind::Kakao, &kakao(false)).email, "");

        let user = extract(
            Kind::Twitter,
            &json!({ "data": { "id": "x1", "username": "xx", "name": "X X",
                               "profile_image_url": "https://a/x.png" } }),
        );
        assert_eq!(
            (
                user.id.as_str(),
                user.username.as_str(),
                user.email.as_str()
            ),
            ("x1", "xx", "")
        );
    }
}

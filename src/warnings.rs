//! Configuration that works as documented but is easy to get wrong.
//!
//! Nothing here changes what slonbase does: a setting is a warning when it is valid and
//! allowed, and its safe use needs something more. The warnings are logged when an installation
//! starts, and `GET /api/settings/warnings` (superusers) lists them for the dashboard.

use serde_json::{Value, json};

use crate::core::Core;

/// One thing worth a second look.
#[derive(Debug, Clone, PartialEq)]
pub struct Warning {
    /// A stable name for what it is about.
    pub code: &'static str,
    /// A short title, and where in the dashboard to change it.
    pub title: &'static str,
    pub link: &'static str,
    /// What it is about: a setting, or a collection and what is set on it.
    pub subject: String,
    pub message: String,
}

impl Warning {
    pub fn json(&self) -> Value {
        json!({
            "code": self.code, "title": self.title, "link": self.link,
            "subject": self.subject, "message": self.message,
        })
    }
}

/// The warnings for the settings and collections `core` has now.
pub fn collect(core: &Core) -> Vec<Warning> {
    let settings = core.settings();
    let registry = core.registry();
    let mut warnings = Vec::new();

    // the client address is taken from headers that any client can send, unless the proxies
    // that may set them are named
    if !settings.trusted_proxy.headers.is_empty() && settings.trusted_proxy.peers.is_empty() {
        warnings.push(Warning {
            code: "proxyPeers",
            title: "Proxy headers are believed from any client",
            link: "#/settings/security",
            subject: "trustedProxy".into(),
            message: "trustedProxy.headers is set without trustedProxy.peers: any client can \
                      send those headers, and so choose the address that rate limits and \
                      sign-in lockouts see. Set trustedProxy.peers to the addresses of your \
                      proxies, unless only the proxy can reach slonbase."
                .into(),
        });
    }

    for collection in registry.all() {
        let Some(auth) = collection.auth_options().filter(|_| !collection.system) else {
            continue;
        };
        let name = &collection.name;

        // an OpenID Connect token is trusted without its signature being checked when no
        // key set is configured
        if auth.oauth2.enabled {
            for provider in auth
                .oauth2
                .providers
                .iter()
                .filter(|p| p.name.starts_with("oidc"))
            {
                let keys = provider
                    .extra
                    .get("jwksURL")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if keys.is_empty() {
                    warnings.push(Warning {
                        code: "oidcSignature",
                        title: "An OpenID Connect token's signature isn't checked",
                        link: "#/schema",
                        subject: format!("{name}: {}", provider.name),
                        message: format!(
                            "The OpenID Connect provider {} of {name} has no extra.jwksURL, so \
                             the signature of its id_token is not checked (the token comes \
                             from the provider over TLS). Set extra.jwksURL to check it, and \
                             extra.requireSignature to refuse the provider without it.",
                            provider.name
                        ),
                    });
                }
            }
        }

        // an empty rule is open to everyone, guests included
        if auth.manage_rule.as_deref() == Some("") {
            warnings.push(Warning {
                code: "openManageRule",
                title: "Anyone can manage accounts",
                link: "#/schema",
                subject: format!("{name}: manageRule"),
                message: format!(
                    "{name} holds user accounts and its manageRule is empty, which allows \
                     anyone, guests included, to change other records' email, password and \
                     verified state. Leave it unset (superusers only) or write a condition."
                ),
            });
        }
        for (rule, value) in [
            ("updateRule", &collection.update_rule),
            ("deleteRule", &collection.delete_rule),
        ] {
            if value.as_deref() == Some("") {
                warnings.push(Warning {
                    code: "openWriteRule",
                    title: "Anyone can change accounts",
                    link: "#/schema",
                    subject: format!("{name}: {rule}"),
                    message: format!(
                        "{name} holds user accounts and its {rule} is empty, which allows \
                         anyone, guests included. Write a condition, such as \
                         `id = @request.auth.id`."
                    ),
                });
            }
        }
    }
    warnings
}

/// Logs the warnings, once each, as an installation starts.
pub fn log(core: &Core) {
    for warning in collect(core) {
        tracing::warn!(code = warning.code, subject = %warning.subject, "{}", warning.message);
    }
}

/// `GET /api/settings/warnings`: the warnings, for the dashboard.
pub async fn list(
    axum::extract::State(app): axum::extract::State<std::sync::Arc<crate::app::App>>,
    auth: crate::auth::RequestAuth,
) -> Result<axum::Json<Value>, crate::error::ApiError> {
    let core = app.core()?;
    auth.require_superuser()?;
    Ok(axum::Json(json!({
        "items": collect(&core).iter().map(Warning::json).collect::<Vec<_>>(),
    })))
}

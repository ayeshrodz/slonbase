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

    // hook code that loops or allocates without end is only stopped when a limit is set
    if core.hooks_unlimited() {
        warnings.push(Warning {
            code: "hookLimits",
            title: "Hook code runs without a time or memory limit",
            link: "",
            subject: "hooks".into(),
            message: "hooks.timeout_seconds and hooks.memory_limit_mb are both 0, so a hook that \
                      loops or allocates without end is not stopped. New installations set \
                      60 seconds and 128 MB; set them in the configuration file, or with \
                      --hooks-timeout and --hooks-memory (SLONBASE_HOOKS_TIMEOUT and \
                      SLONBASE_HOOKS_MEMORY)."
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

/// What the last check of the stored templates and automations found, and what it was made
/// against.
#[derive(Default)]
pub struct StoredCheck {
    found: std::sync::Arc<Vec<Warning>>,
    /// The registry and settings generations it was made against, and when.
    made: Option<(u64, u64, std::time::Instant)>,
    running: bool,
}

/// The longest a check of the stored definitions is kept when nothing changed (a stored
/// address may stop resolving).
const STORED_CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

/// The templates and automations that no longer pass the rules they are saved under (a rule
/// that became stricter, a collection or field they use that was renamed or removed): they are
/// found when they run, otherwise. Nothing is changed or switched off. The answer is the last
/// check's; a new one runs in the background when the settings or the collections changed, or
/// it is old.
pub fn stored(core: &std::sync::Arc<Core>) -> std::sync::Arc<Vec<Warning>> {
    let (found, stale) = {
        let state = core.stored_check.lock().expect("stored check lock");
        let stale = !state.running
            && match state.made {
                None => true,
                Some((registry, settings, at)) => {
                    registry != core.registry_generation()
                        || settings != core.settings_generation()
                        || at.elapsed() > STORED_CHECK_EVERY
                }
            };
        (state.found.clone(), stale)
    };
    if stale {
        check_stored_in_background(core, false);
    }
    found
}

/// Starts a check of the stored templates and automations (when none runs); `log` writes what
/// it finds to the log.
pub fn check_stored_in_background(core: &std::sync::Arc<Core>, log: bool) {
    {
        let mut state = core.stored_check.lock().expect("stored check lock");
        if state.running {
            return;
        }
        state.running = true;
    }
    let core = core.clone();
    tokio::spawn(async move {
        let registry = core.registry_generation();
        let settings = core.settings_generation();
        let found = check_stored(&core).await;
        if log {
            for warning in &found {
                tracing::warn!(code = warning.code, subject = %warning.subject, "{}", warning.message);
            }
        }
        let mut state = core.stored_check.lock().expect("stored check lock");
        state.found = std::sync::Arc::new(found);
        state.made = Some((registry, settings, std::time::Instant::now()));
        state.running = false;
    });
}

/// Checks every stored template and automation as it would be saved now.
async fn check_stored(core: &std::sync::Arc<Core>) -> Vec<Warning> {
    let settings = core.settings();
    let mut found = Vec::new();
    let what = |errors: &crate::validation::Errors| {
        errors
            .iter()
            .map(|(path, issue)| format!("{path}: {}", issue.message))
            .collect::<Vec<_>>()
            .join(" ")
    };
    for template in &settings.templates {
        let errors = crate::templates::validate(template, &settings.templates);
        if !errors.is_empty() {
            found.push(Warning {
                code: "invalidTemplate",
                title: "A template no longer passes its checks",
                link: "#/automations",
                subject: format!("template {}", template.name),
                message: format!(
                    "The template {:?} would not be saved as it is now: {} Sending it may fail until it is changed.",
                    template.name,
                    what(&errors)
                ),
            });
        }
    }
    for job in &settings.cron_jobs {
        let errors = crate::cron_jobs::validate(core, job, &settings.cron_jobs).await;
        if !errors.is_empty() {
            found.push(Warning {
                code: "invalidAutomation",
                title: "An automation no longer passes its checks",
                link: "#/automations",
                subject: format!("automation {}", job.name),
                message: format!(
                    "The automation {:?} would not be saved as it is now: {} Its runs may fail until it is changed.",
                    job.name,
                    what(&errors)
                ),
            });
        }
    }
    found
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
    let mut items: Vec<Value> = collect(&core).iter().map(Warning::json).collect();
    items.extend(stored(&core).iter().map(Warning::json));
    Ok(axum::Json(json!({ "items": items })))
}

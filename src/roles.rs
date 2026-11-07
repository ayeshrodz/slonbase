//! Dashboard roles and API keys (Settings → Team): what each superuser may do, and keys that
//! act for a superuser with a role of their own. Each installation (the main one and every
//! project) has its own superusers, so its own roles and keys.
//!
//! - **Owner**: everything. A superuser without a role is an owner (as every superuser was
//!   before 2.0, and as in PocketBase).
//! - **Admin**: everything but managing superusers and their roles, restoring backups and
//!   deleting projects.
//! - **Developer**: collections, records, files, the SQL console (reading, or writing when the
//!   server allows it and they're admins), running cron jobs; settings, backups and branding
//!   only to look at.
//! - **Viewer**: looks at everything (no backups to download), changes nothing.
//!
//! The check is one place, [`allowed`], run by the request guard for every request by a
//! superuser who isn't an owner, so routes added by extensions are covered too.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::app::App;
use crate::auth::{AuthRecord, RequestAuth};
use crate::core::Core;
use crate::error::ApiError;
use crate::extract::JsonObject;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Developer,
    Admin,
    Owner,
}

impl Role {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "developer" => Some(Self::Developer),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Developer => "developer",
            Self::Viewer => "viewer",
        }
    }
}

/// An API key: a credential that acts for a superuser (`superuser`), with `role` at most.
/// Only the SHA-256 of the secret is kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    /// The key's first characters (`sb_xxxxxxxx`), shown to tell keys apart.
    pub prefix: String,
    pub hash: String,
    pub role: Role,
    pub superuser: String,
    pub created: String,
    /// Empty: never expires.
    #[serde(default)]
    pub expires: String,
    /// IPs or ranges the key works from (empty: any).
    #[serde(default)]
    pub allowed_ips: Vec<String>,
}

const KEY_PREFIX: &str = "sb_";
const MAX_KEYS: usize = 50;

/// A superuser's role (owner when none is set).
pub fn role_of(core: &Core, superuser_id: &str) -> Role {
    core.settings()
        .superuser_roles
        .get(superuser_id)
        .and_then(|r| Role::parse(r))
        .unwrap_or(Role::Owner)
}

fn superuser_action(rest: &str) -> bool {
    // signing in and the like: everyone may, for themselves
    // (revoking tokens: the handler allows it for the superuser themselves, or an owner)
    (!rest.starts_with("records") || rest.ends_with("/revoke-tokens"))
        && !rest.starts_with("impersonate")
}

/// The path with a collection's id replaced by its name (`/api/collections/pbc_…/records` becomes
/// `/api/collections/_superusers/records`), so that a role's rules, which are written for names,
/// apply however the collection is addressed.
pub fn canonical_path(path: &str, registry: &crate::collections::store::Registry) -> String {
    const PREFIX: &str = "/api/collections/";
    if let Some(rest) = path.strip_prefix(PREFIX) {
        let (segment, tail) = match rest.split_once('/') {
            Some((segment, tail)) => (segment, Some(tail)),
            None => (rest, None),
        };
        if let Some(collection) = registry.get(segment) {
            if collection.name != segment {
                return match tail {
                    Some(tail) => format!("{PREFIX}{}/{tail}", collection.name),
                    None => format!("{PREFIX}{}", collection.name),
                };
            }
        }
    }
    path.to_string()
}

/// Whether `role` may make a request (`method` and the installation's `path`).
pub fn allowed(role: Role, method: &Method, path: &str) -> bool {
    allowed_for(role, method, path, false)
}

/// [`allowed`], for a credential that may be an API key (`via_key`). A key acts for its superuser
/// with at most its own role, so the things every superuser may do for themselves (changing their
/// password, ending their sessions) are not its to do: a read-only key must not be able to take
/// over the account it belongs to.
pub fn allowed_for(role: Role, method: &Method, path: &str, via_key: bool) -> bool {
    if role == Role::Owner {
        return true;
    }
    let read = matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    // the whole installation in one file: owners and admins
    let backup_file = path.starts_with("/api/backups/")
        && !path.ends_with("/restore")
        && path.matches('/').count() == 3;
    if read {
        return !(backup_file && role < Role::Admin);
    }
    // what every superuser does for themselves
    if !via_key {
        if let Some(rest) = path.strip_prefix("/api/collections/_superusers/") {
            if superuser_action(rest) {
                return true;
            }
        }
    }
    if path == "/api/realtime" || path == "/api/files/token" || path == "/api/team/me" {
        return true;
    }
    // (a key reaches every other path only through its role below)
    match role {
        Role::Owner => true,
        Role::Viewer => false,
        Role::Developer => {
            let data = path.starts_with("/api/collections")
                && !path.starts_with("/api/collections/_superusers")
                && !path.contains("/impersonate/");
            data || path == "/api/batch"
                || path.starts_with("/api/crons/")
                || path == "/api/settings/mail/preview"
        }
        Role::Admin => {
            let superusers = path.starts_with("/api/collections/_superusers");
            let roles = path.starts_with("/api/team/members");
            let owner_keys = false; // checked when the key is made
            let restore = path.starts_with("/api/backups/") && path.ends_with("/restore");
            let delete_project = *method == Method::DELETE && path.starts_with("/api/projects/");
            // the console reads every table, the signing secret and password hashes included
            let console = path == "/api/sql";
            !(superusers || roles || owner_keys || restore || delete_project || console)
        }
    }
}

// ---------------------------------------------------------------------------
// API keys

fn hash(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

fn last_used() -> &'static Mutex<HashMap<String, String>> {
    static LAST: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    LAST.get_or_init(Default::default)
}

pub fn is_api_key(token: &str) -> bool {
    token.starts_with(KEY_PREFIX)
}

/// The superuser an API key acts for, and its role, if the key is valid here and now.
pub async fn authenticate_key(
    core: &Core,
    token: &str,
    ip: Option<std::net::IpAddr>,
) -> Option<(AuthRecord, Role)> {
    let settings = core.settings();
    let digest = hash(token);
    let key = settings
        .api_keys
        .iter()
        .find(|k| crate::ids::constant_time_eq(&k.hash, &digest))?;
    if !key.expires.is_empty() {
        let expires = chrono::DateTime::parse_from_rfc3339(&key.expires).ok()?;
        if expires < chrono::Utc::now() {
            return None;
        }
    }
    if !key.allowed_ips.is_empty() && !ip.is_some_and(|ip| crate::ip::in_any(&key.allowed_ips, ip))
    {
        return None;
    }
    let registry = core.registry();
    let collection = registry.get(crate::collections::model::SUPERUSERS_COLLECTION_NAME)?;
    let client = core.pool.get().await.ok()?;
    let condition = format!("{}.id = $1", crate::records::store::ALIAS);
    let values = crate::records::store::find_by(&client, collection, &condition, &key.superuser)
        .await
        .ok()??;
    let record = AuthRecord {
        collection: collection.clone(),
        values,
        refreshable: false,
    };
    // never more than the superuser it acts for
    let role = key.role.min(role_of(core, &key.superuser));
    last_used()
        .lock()
        .expect("keys lock")
        .insert(key.id.clone(), crate::datetime::format(&chrono::Utc::now()));
    Some((record, role))
}

// ---------------------------------------------------------------------------
// the API

fn require_role(
    core: &Core,
    auth: &RequestAuth,
    min: Role,
) -> Result<(AuthRecord, Role), ApiError> {
    let me = auth.require_superuser()?.clone();
    let role = auth.role.unwrap_or_else(|| role_of(core, me.id()));
    if role < min {
        return Err(ApiError::forbidden(format!(
            "Only {}s and owners can do this.",
            min.as_str()
        )));
    }
    Ok((me, role))
}

async fn save_settings(
    core: &Arc<Core>,
    settings: crate::settings::Settings,
) -> Result<(), ApiError> {
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    settings
        .save(&client, core.secrets_key())
        .await
        .map_err(ApiError::internal)?;
    crate::realtime::notify(&client, "settings")
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    core.set_settings(settings);
    Ok(())
}

fn key_json(k: &ApiKey) -> Value {
    json!({
        "id": k.id,
        "name": k.name,
        "prefix": k.prefix,
        "role": k.role,
        "superuser": k.superuser,
        "created": k.created,
        "expires": k.expires,
        "allowedIPs": k.allowed_ips,
        "lastUsed": last_used().lock().expect("keys lock").get(&k.id),
    })
}

/// `GET /api/team/me`: the signed-in superuser's role.
pub async fn me(State(app): State<Arc<App>>, auth: RequestAuth) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let (me, role) = require_role(&core, &auth, Role::Viewer)?;
    Ok(Json(
        json!({ "id": me.id(), "email": me.email(), "role": role }),
    ))
}

/// `GET /api/team`: the superusers with their roles, and the API keys.
pub async fn list(State(app): State<Arc<App>>, auth: RequestAuth) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    require_role(&core, &auth, Role::Viewer)?;
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    let rows = client
        .query(
            "SELECT id, email, verified, created FROM slonbase._superusers ORDER BY created",
            &[],
        )
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    let settings = core.settings();
    let members: Vec<Value> = rows
        .iter()
        .map(|r| {
            let id: String = r.get("id");
            let created: chrono::DateTime<chrono::Utc> = r.get("created");
            json!({
                "id": id,
                "email": r.get::<_, String>("email"),
                "verified": r.get::<_, bool>("verified"),
                "created": crate::datetime::format(&created),
                "role": role_of(&core, &id),
            })
        })
        .collect();
    Ok(Json(json!({
        "members": members,
        "apiKeys": settings.api_keys.iter().map(key_json).collect::<Vec<_>>(),
    })))
}

/// `PATCH /api/team/members/{id}` (`{"role"}`): owners change roles; there's always an owner.
pub async fn set_role(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    Path(id): Path<String>,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    require_role(&core, &auth, Role::Owner)?;
    let role = body
        .get("role")
        .and_then(Value::as_str)
        .and_then(Role::parse)
        .ok_or_else(|| ApiError::bad_request("role must be owner, admin, developer or viewer."))?;
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    let ids: Vec<String> = client
        .query("SELECT id FROM slonbase._superusers", &[])
        .await
        .map_err(|e| ApiError::internal(e.into()))?
        .iter()
        .map(|r| r.get(0))
        .collect();
    if !ids.contains(&id) {
        return Err(ApiError::not_found());
    }
    let mut settings = (*core.settings()).clone();
    if role == Role::Owner {
        settings.superuser_roles.remove(&id);
    } else {
        settings
            .superuser_roles
            .insert(id.clone(), role.as_str().to_string());
    }
    // roles of superusers that are gone go too
    settings.superuser_roles.retain(|k, _| ids.contains(k));
    let owners = ids
        .iter()
        .filter(|i| !settings.superuser_roles.contains_key(*i))
        .count();
    if owners == 0 {
        return Err(ApiError::bad_request("There must be at least one owner."));
    }
    save_settings(&core, settings).await?;
    tracing::info!(superuser = %id, role = role.as_str(), "changed a superuser's role");
    Ok(Json(json!({ "id": id, "role": role })))
}

/// Refuses deleting the last owner (called by the guard for `DELETE` of a superuser).
pub async fn check_delete(core: &Core, id: &str) -> Result<(), ApiError> {
    if role_of(core, id) != Role::Owner {
        return Ok(());
    }
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    let ids: Vec<String> = client
        .query("SELECT id FROM slonbase._superusers", &[])
        .await
        .map_err(|e| ApiError::internal(e.into()))?
        .iter()
        .map(|r| r.get(0))
        .collect();
    let roles = &core.settings().superuser_roles;
    let other_owners = ids
        .iter()
        .filter(|i| *i != id && !roles.contains_key(*i))
        .count();
    if other_owners == 0 {
        return Err(ApiError::bad_request(
            "This is the last owner: make another superuser an owner first.",
        ));
    }
    Ok(())
}

/// `POST /api/team/keys` (`{name, role, expiresDays?, allowedIPs?}`): a new API key for the
/// signed-in superuser, with at most their role. The key is in the answer, once.
pub async fn create_key(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    let core = app.core()?;
    let (me, my_role) = require_role(&core, &auth, Role::Admin)?;
    let mut errors = crate::validation::Errors::new();
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if name.is_empty() || name.chars().count() > 60 {
        errors.invalid("name", "Must be 1 to 60 characters.");
    }
    let role = body
        .get("role")
        .and_then(Value::as_str)
        .and_then(Role::parse)
        .unwrap_or(Role::Viewer);
    if role > my_role {
        errors.invalid("role", "Can't be more than your own role.");
    }
    let days = body.get("expiresDays").and_then(Value::as_i64).unwrap_or(0);
    if !(0..=3650).contains(&days) {
        errors.invalid("expiresDays", "Must be between 0 (never) and 3650.");
    }
    let allowed_ips: Vec<String> = body
        .get("allowedIPs")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    for (i, ip) in allowed_ips.iter().enumerate() {
        if crate::ip::Range::parse(ip).is_none() {
            errors.invalid(
                format!("allowedIPs.{i}"),
                "Must be an IP address or a CIDR range.",
            );
        }
    }
    let mut settings = (*core.settings()).clone();
    if settings.api_keys.len() >= MAX_KEYS {
        errors.invalid(
            "name",
            "An installation has at most 50 keys; revoke some first.",
        );
    }
    if !errors.is_empty() {
        return Err(ApiError::invalid_data(
            "An error occurred while validating the submitted data.",
            &errors,
        ));
    }
    let secret = format!("{KEY_PREFIX}{}", crate::ids::random_id(40));
    let now = chrono::Utc::now();
    let key = ApiKey {
        id: crate::ids::random_id(15),
        name,
        prefix: secret[..KEY_PREFIX.len() + 8].to_string(),
        hash: hash(&secret),
        role,
        superuser: me.id().to_string(),
        created: crate::datetime::format(&now),
        expires: if days > 0 {
            (now + chrono::Duration::days(days)).to_rfc3339()
        } else {
            String::new()
        },
        allowed_ips,
    };
    let mut answer = key_json(&key);
    answer["key"] = secret.into();
    settings.api_keys.push(key);
    save_settings(&core, settings).await?;
    Ok(Json(answer))
}

/// `DELETE /api/team/keys/{id}`: revokes a key (its own superuser's, or any for owners).
pub async fn delete_key(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let core = app.core()?;
    let (me, role) = require_role(&core, &auth, Role::Admin)?;
    let mut settings = (*core.settings()).clone();
    let Some(pos) = settings.api_keys.iter().position(|k| k.id == id) else {
        return Err(ApiError::not_found());
    };
    if role != Role::Owner && settings.api_keys[pos].superuser != me.id() {
        return Err(ApiError::forbidden("Only owners can revoke others' keys."));
    }
    settings.api_keys.remove(pos);
    save_settings(&core, settings).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions() {
        let can = |role, method: Method, path: &str| allowed(role, &method, path);
        for role in [Role::Viewer, Role::Developer, Role::Admin, Role::Owner] {
            assert!(can(role, Method::GET, "/api/collections"));
            assert!(can(role, Method::GET, "/api/settings"));
            assert!(can(
                role,
                Method::POST,
                "/api/collections/_superusers/auth-refresh"
            ));
            assert!(can(role, Method::POST, "/api/realtime"));
        }
        // viewers change nothing, and don't download backups
        assert!(!can(
            Role::Viewer,
            Method::POST,
            "/api/collections/posts/records"
        ));
        assert!(!can(Role::Viewer, Method::POST, "/api/sql"));
        assert!(!can(Role::Viewer, Method::GET, "/api/backups/x.zip"));
        assert!(can(Role::Viewer, Method::GET, "/api/backups"));
        // developers work on the data
        assert!(can(
            Role::Developer,
            Method::POST,
            "/api/collections/posts/records"
        ));
        assert!(can(
            Role::Developer,
            Method::PATCH,
            "/api/collections/posts"
        ));
        // the SQL console reads everything, the signing secret included: owners only
        assert!(!can(Role::Developer, Method::POST, "/api/sql"));
        assert!(!can(Role::Admin, Method::POST, "/api/sql"));
        assert!(can(Role::Owner, Method::POST, "/api/sql"));
        assert!(can(Role::Developer, Method::POST, "/api/crons/report"));
        assert!(!can(Role::Developer, Method::PATCH, "/api/settings"));
        assert!(!can(Role::Developer, Method::POST, "/api/backups"));
        assert!(!can(
            Role::Developer,
            Method::POST,
            "/api/collections/_superusers/records"
        ));
        assert!(!can(
            Role::Developer,
            Method::POST,
            "/api/collections/users/impersonate/x"
        ));
        assert!(!can(Role::Developer, Method::POST, "/api/branding/assets"));
        // admins all but superusers, roles, restores and deleting projects
        assert!(can(Role::Admin, Method::PATCH, "/api/settings"));
        assert!(can(Role::Admin, Method::POST, "/api/backups"));
        assert!(can(Role::Admin, Method::GET, "/api/backups/x.zip"));
        assert!(can(Role::Admin, Method::POST, "/api/team/keys"));
        assert!(!can(
            Role::Admin,
            Method::POST,
            "/api/backups/x.zip/restore"
        ));
        assert!(!can(
            Role::Admin,
            Method::DELETE,
            "/api/collections/_superusers/records/x"
        ));
        assert!(!can(Role::Admin, Method::PATCH, "/api/team/members/x"));
        assert!(!can(Role::Admin, Method::DELETE, "/api/projects/blog"));
        assert!(can(Role::Owner, Method::POST, "/api/backups/x.zip/restore"));
    }

    /// Every API route, by method, with the lowest role that may use it (`None`: no role may,
    /// the request needs another credential). A route added to `server.rs` without a line here
    /// fails `every_route_has_a_role_decision`, so each new route gets a deliberate answer
    /// (and a changed answer shows up in review as a changed line).
    const POLICY: &[(&str, &str, Option<Role>)] = &[
        ("DELETE", "/api/backups/{key}", Some(Role::Admin)),
        ("DELETE", "/api/branding/assets/{id}", Some(Role::Admin)),
        (
            "DELETE",
            "/api/collections/{collection}",
            Some(Role::Developer),
        ),
        (
            "DELETE",
            "/api/collections/{collection}/records/{id}",
            Some(Role::Developer),
        ),
        (
            "DELETE",
            "/api/collections/{collection}/records/{id}/external-auths/{provider}",
            Some(Role::Developer),
        ),
        (
            "DELETE",
            "/api/collections/{collection}/truncate",
            Some(Role::Developer),
        ),
        ("DELETE", "/api/logs", Some(Role::Admin)),
        ("DELETE", "/api/projects/{name}", Some(Role::Owner)),
        ("DELETE", "/api/team/keys/{id}", Some(Role::Admin)),
        ("GET", "/api/backups", Some(Role::Viewer)),
        ("GET", "/api/backups/{key}", Some(Role::Admin)),
        ("GET", "/api/branding/assets", Some(Role::Viewer)),
        ("GET", "/api/branding/assets/{id}", Some(Role::Viewer)),
        ("GET", "/api/collections", Some(Role::Viewer)),
        (
            "GET",
            "/api/collections/meta/oauth2-providers",
            Some(Role::Viewer),
        ),
        ("GET", "/api/collections/meta/scaffolds", Some(Role::Viewer)),
        ("GET", "/api/collections/{collection}", Some(Role::Viewer)),
        (
            "GET",
            "/api/collections/{collection}/auth-methods",
            Some(Role::Viewer),
        ),
        (
            "GET",
            "/api/collections/{collection}/records",
            Some(Role::Viewer),
        ),
        (
            "GET",
            "/api/collections/{collection}/records/{id}",
            Some(Role::Viewer),
        ),
        (
            "GET",
            "/api/collections/{collection}/records/{id}/external-auths",
            Some(Role::Viewer),
        ),
        ("GET", "/api/crons", Some(Role::Viewer)),
        (
            "GET",
            "/api/files/{collection}/{record}/{filename}",
            Some(Role::Viewer),
        ),
        ("GET", "/api/health", Some(Role::Viewer)),
        ("GET", "/api/jobs/runs", Some(Role::Viewer)),
        ("GET", "/api/logs", Some(Role::Viewer)),
        ("GET", "/api/logs/stats", Some(Role::Viewer)),
        ("GET", "/api/logs/{id}", Some(Role::Viewer)),
        ("GET", "/api/oauth2-redirect", Some(Role::Viewer)),
        ("GET", "/api/projects", Some(Role::Viewer)),
        ("GET", "/api/projects/overview", Some(Role::Viewer)),
        ("GET", "/api/projects/{name}", Some(Role::Viewer)),
        ("GET", "/api/projects/{name}/settings", Some(Role::Viewer)),
        ("GET", "/api/realtime", Some(Role::Viewer)),
        ("GET", "/api/settings", Some(Role::Viewer)),
        ("GET", "/api/settings/storage", Some(Role::Viewer)),
        ("GET", "/api/setup", Some(Role::Viewer)),
        ("GET", "/api/sql", Some(Role::Viewer)),
        ("GET", "/api/system/status", Some(Role::Viewer)),
        ("GET", "/api/team", Some(Role::Viewer)),
        ("GET", "/api/team/me", Some(Role::Viewer)),
        ("PATCH", "/api/branding/assets/{id}", Some(Role::Admin)),
        (
            "PATCH",
            "/api/collections/{collection}",
            Some(Role::Developer),
        ),
        (
            "PATCH",
            "/api/collections/{collection}/records/{id}",
            Some(Role::Developer),
        ),
        ("PATCH", "/api/projects/{name}", Some(Role::Admin)),
        ("PATCH", "/api/projects/{name}/settings", Some(Role::Admin)),
        ("PATCH", "/api/settings", Some(Role::Admin)),
        ("PATCH", "/api/team/members/{id}", Some(Role::Owner)),
        ("POST", "/api/backups", Some(Role::Admin)),
        ("POST", "/api/backups/upload", Some(Role::Admin)),
        ("POST", "/api/backups/{key}/restore", Some(Role::Owner)),
        ("POST", "/api/batch", Some(Role::Developer)),
        ("POST", "/api/branding/assets", Some(Role::Admin)),
        ("POST", "/api/collections", Some(Role::Developer)),
        (
            "POST",
            "/api/collections/meta/dry-run-view",
            Some(Role::Developer),
        ),
        (
            "POST",
            "/api/collections/{collection}/auth-refresh",
            Some(Role::Developer),
        ),
        (
            "POST",
            "/api/collections/{collection}/auth-with-oauth2",
            Some(Role::Developer),
        ),
        (
            "POST",
            "/api/collections/{collection}/auth-with-otp",
            Some(Role::Developer),
        ),
        (
            "POST",
            "/api/collections/{collection}/auth-with-password",
            Some(Role::Developer),
        ),
        (
            "POST",
            "/api/collections/{collection}/confirm-email-change",
            Some(Role::Developer),
        ),
        (
            "POST",
            "/api/collections/{collection}/confirm-password-reset",
            Some(Role::Developer),
        ),
        (
            "POST",
            "/api/collections/{collection}/confirm-verification",
            Some(Role::Developer),
        ),
        (
            "POST",
            "/api/collections/{collection}/impersonate/{id}",
            Some(Role::Admin),
        ),
        (
            "POST",
            "/api/collections/{collection}/records",
            Some(Role::Developer),
        ),
        (
            "POST",
            "/api/collections/{collection}/records/{id}/revoke-tokens",
            Some(Role::Developer),
        ),
        (
            "POST",
            "/api/collections/{collection}/request-email-change",
            Some(Role::Developer),
        ),
        (
            "POST",
            "/api/collections/{collection}/request-otp",
            Some(Role::Developer),
        ),
        (
            "POST",
            "/api/collections/{collection}/request-password-reset",
            Some(Role::Developer),
        ),
        (
            "POST",
            "/api/collections/{collection}/request-verification",
            Some(Role::Developer),
        ),
        ("POST", "/api/crons/{id}", Some(Role::Developer)),
        ("POST", "/api/files/token", Some(Role::Viewer)),
        ("POST", "/api/oauth2-redirect", Some(Role::Admin)),
        ("POST", "/api/projects", Some(Role::Admin)),
        ("POST", "/api/projects/{name}/restart", Some(Role::Admin)),
        ("POST", "/api/realtime", Some(Role::Viewer)),
        (
            "POST",
            "/api/settings/apple/generate-client-secret",
            Some(Role::Admin),
        ),
        ("POST", "/api/settings/mail/preview", Some(Role::Developer)),
        (
            "POST",
            "/api/settings/rate-limits/explain",
            Some(Role::Admin),
        ),
        (
            "POST",
            "/api/settings/storage/clear-thumbs",
            Some(Role::Admin),
        ),
        ("POST", "/api/settings/test/email", Some(Role::Admin)),
        ("POST", "/api/settings/test/s3", Some(Role::Admin)),
        ("POST", "/api/setup/complete", Some(Role::Admin)),
        ("POST", "/api/setup/database/test", Some(Role::Admin)),
        ("POST", "/api/sql", Some(Role::Owner)),
        ("POST", "/api/team/keys", Some(Role::Admin)),
        ("PUT", "/api/collections/import", Some(Role::Developer)),
    ];

    /// The `(method, path)` of every `/api/` route registered in `server.rs`.
    fn routes() -> Vec<(String, String)> {
        let source = include_str!("server.rs");
        let path = regex::Regex::new(r#"\.route\(\s*"(/api/[^"]+)""#).unwrap();
        let method = regex::Regex::new(r"\b(get|post|put|patch|delete)\(").unwrap();
        let starts: Vec<_> = path.captures_iter(source).collect();
        let mut out = Vec::new();
        for (n, caps) in starts.iter().enumerate() {
            let whole = caps.get(0).unwrap();
            let end = starts
                .get(n + 1)
                .map(|c| c.get(0).unwrap().start())
                .unwrap_or(source.len());
            // up to the next route, or the end of this router chain
            let chunk = &source[whole.end()..end];
            let chunk = chunk.split(".merge(").next().unwrap();
            for m in method.captures_iter(chunk) {
                out.push((m[1].to_uppercase(), caps[1].to_string()));
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn sample(path: &str) -> String {
        path.replace("{collection}", "posts")
            .replace("{name}", "x")
            .replace("{id}", "x")
            .replace("{key}", "x")
            .replace("{provider}", "x")
            .replace("{record}", "x")
            .replace("{filename}", "x")
    }

    fn lowest(method: &Method, path: &str) -> Option<Role> {
        [Role::Viewer, Role::Developer, Role::Admin, Role::Owner]
            .into_iter()
            .find(|role| allowed(*role, method, &sample(path)))
    }

    #[test]
    fn every_route_has_a_role_decision() {
        let routes = routes();
        assert!(routes.len() > 60, "found only {} routes", routes.len());
        if std::env::var("PRINT_POLICY").is_ok() {
            for (m, p) in &routes {
                let min = lowest(&m.parse().unwrap(), p);
                println!("POLICY: (\"{m}\", \"{p}\", {min:?}),");
            }
        }
        let mut missing = Vec::new();
        for (m, p) in &routes {
            match POLICY.iter().find(|(pm, pp, _)| pm == m && pp == p) {
                None => missing.push(format!("{m} {p}")),
                Some((_, _, min)) => assert_eq!(
                    lowest(&m.parse().unwrap(), p),
                    *min,
                    "{m} {p}: the role rules changed; update POLICY on purpose"
                ),
            }
        }
        assert!(
            missing.is_empty(),
            "routes without a role decision: {missing:?}"
        );
        for (m, p, _) in POLICY {
            assert!(
                routes.iter().any(|(rm, rp)| rm == m && rp == p),
                "POLICY lists {m} {p}, which isn't a route any more"
            );
        }
    }
}

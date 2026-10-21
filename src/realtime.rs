//! Realtime: record change events delivered over Server-Sent Events, compatible with
//! PocketBase's `/api/realtime` protocol and SDKs.
//!
//! Record changes are written to an outbox (`_slonbase.events`) and announced with
//! `pg_notify` in the same transaction, so rolled-back changes are never delivered. Every
//! instance listens on the channel, so subscribers get changes made through any
//! instance. The same channel tells instances to reload collections and settings.
//!
//! Access is checked per subscriber with the collection's `listRule` (for `collection/*`
//! topics) or `viewRule` (for `collection/{id}` topics), evaluated in one SQL query per
//! event against the event's record values (which also works for deleted records).

use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use deadpool_postgres::GenericClient;
use futures_util::StreamExt;
use futures_util::stream;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::app::App;
use crate::auth::RequestAuth;
use crate::collections::model::{Collection, RuleAction};
use crate::collections::store;
use crate::config::DatabaseConfig;
use crate::core::Core;
use crate::db::TextQuery;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::filter::{Params, RequestInfo, Scope, quote_ident, to_sql};
use crate::records::access::{email_visible_sql, pick_fields, rule_sql};
use crate::records::store::{ALIAS, record_json, visible_fields};
use crate::records::values::{self, Values};
use crate::settings::Settings;
use crate::{db, ids};

/// The notification channel shared by all instances.
pub const CHANNEL: &str = "slonbase";

const CLIENT_BUFFER: usize = 512;
const MAX_SUBSCRIPTIONS: usize = 1000;
const MAX_TOPIC_LENGTH: usize = 2500;
/// Outbox rows are kept this long (for catching up after a listener reconnect).
const EVENT_RETENTION_SECS: i64 = 300;
/// Conditions evaluated per access check query.
const CHECK_CHUNK: usize = 50;
/// The topic of OAuth2 redirects (the SDKs' popup sign-in flow).
pub const OAUTH2_TOPIC: &str = "@oauth2";
/// OTPs and MFA sessions are removed this long after creation (their longest duration).
const AUTH_RETENTION_SECS: i64 = 86_400;

// ---------------------------------------------------------------------------
// outbox

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Create,
    Update,
    Delete,
}

impl Action {
    fn as_str(self) -> &'static str {
        match self {
            Action::Create => "create",
            Action::Update => "update",
            Action::Delete => "delete",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "create" => Some(Action::Create),
            "update" => Some(Action::Update),
            "delete" => Some(Action::Delete),
            _ => None,
        }
    }
}

/// Records a change in the outbox. Call inside the transaction that makes the change.
pub async fn record_event(
    client: &impl db::TextQuery,
    c: &Collection,
    action: Action,
    record: &Values,
) -> Result<(), tokio_postgres::Error> {
    // secrets never leave the record table
    let mut stored = record.clone();
    for field in c.fields.iter().filter(|f| f.is_secret()) {
        stored.remove(&field.name);
    }
    use tokio_postgres::types::Type;
    // one statement, sent with its parameter types: one round trip per write
    let action = action.as_str().to_string();
    let record = Value::Object(stored);
    tokio_postgres::GenericClient::query_typed(
        client.inner(),
        "WITH e AS (INSERT INTO _slonbase.events (collection_id, action, record) \
             VALUES ($1, $2, $3) RETURNING id) \
             SELECT pg_notify($4, 'event:' || e.id) FROM e",
        &[
            (
                &c.id as &(dyn tokio_postgres::types::ToSql + Sync),
                Type::TEXT,
            ),
            (&action, Type::TEXT),
            (&record, Type::JSONB),
            (&CHANNEL, Type::TEXT),
        ],
    )
    .await?;
    Ok(())
}

/// Announces a change of collections or settings to all instances.
pub async fn notify(client: &impl GenericClient, what: &str) -> Result<(), tokio_postgres::Error> {
    client
        .execute("SELECT pg_notify($1, $2)", &[&CHANNEL, &what])
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// clients and subscriptions

#[derive(Debug, Clone)]
struct Topic {
    /// The subscription as sent by the client (also the SSE event name).
    raw: String,
    collection: String,
    /// `None` for collection-wide topics.
    record_id: Option<String>,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
}

impl Topic {
    fn parse(raw: &str) -> Option<Self> {
        let (path, query_string) = raw.split_once('?').unwrap_or((raw, ""));
        let (collection, record_id) = match path.split_once('/') {
            Some((c, "*")) => (c, None),
            Some((c, id)) if !id.is_empty() && !id.contains('/') => (c, Some(id.to_string())),
            Some(_) => return None,
            None => (path, None),
        };
        if collection.is_empty() {
            return None;
        }

        let mut query = HashMap::new();
        let mut headers = HashMap::new();
        for (key, value) in form_urlencoded::parse(query_string.as_bytes()) {
            if key != "options" {
                continue;
            }
            let options: Value = serde_json::from_str(&value).ok()?;
            let to_map = |v: Option<&Value>| -> HashMap<String, String> {
                v.and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| {
                                let v = match v {
                                    Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                };
                                (k.clone(), v)
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            };
            query = to_map(options.get("query"));
            headers = to_map(options.get("headers"))
                .into_iter()
                .map(|(k, v)| (k.to_ascii_lowercase().replace('-', "_"), v))
                .collect();
        }

        Some(Self {
            raw: raw.to_string(),
            collection: collection.to_string(),
            record_id,
            query,
            headers,
        })
    }
}

struct Message {
    event: String,
    data: String,
}

#[derive(Default)]
struct Subscriptions {
    /// The auth state set by the last subscribe request (`None` = guest).
    auth: Option<RequestAuth>,
    topics: Vec<Topic>,
}

struct Client {
    sender: mpsc::Sender<Message>,
    subscriptions: Mutex<Subscriptions>,
}

#[derive(Default)]
pub struct Hub {
    clients: Mutex<HashMap<String, Arc<Client>>>,
}

impl Hub {
    fn register(&self) -> (String, mpsc::Receiver<Message>) {
        let id = ids::random_id(40);
        let (sender, receiver) = mpsc::channel(CLIENT_BUFFER);
        self.clients.lock().expect("hub lock").insert(
            id.clone(),
            Arc::new(Client {
                sender,
                subscriptions: Mutex::new(Subscriptions::default()),
            }),
        );
        (id, receiver)
    }

    fn remove(&self, id: &str) {
        self.clients.lock().expect("hub lock").remove(id);
    }

    fn get(&self, id: &str) -> Option<Arc<Client>> {
        self.clients.lock().expect("hub lock").get(id).cloned()
    }

    fn snapshot(&self) -> Vec<(String, Arc<Client>)> {
        self.clients
            .lock()
            .expect("hub lock")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Disconnects every client (their streams end).
    pub fn disconnect_all(&self) {
        self.clients.lock().expect("hub lock").clear();
    }

    pub fn client_count(&self) -> usize {
        self.clients.lock().expect("hub lock").len()
    }
}

/// Removes the client from the hub when its SSE stream is dropped (disconnect).
struct ClientGuard {
    hub: Weak<Core>,
    id: String,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        if let Some(core) = self.hub.upgrade() {
            core.realtime.remove(&self.id);
            tracing::debug!(client = %self.id, "realtime client disconnected");
        }
    }
}

/// `GET /api/realtime`: opens the event stream. The first event is `PB_CONNECT`
/// with the client id used to set subscriptions.
pub async fn connect(State(app): State<Arc<App>>) -> Result<impl IntoResponse, ApiError> {
    let core = app.core()?;
    let (id, receiver) = core.realtime.register();
    let guard = ClientGuard {
        hub: Arc::downgrade(&core),
        id: id.clone(),
    };
    let connect = Event::default()
        .id(id.clone())
        .event("PB_CONNECT")
        .data(json!({ "clientId": id }).to_string());

    let events = stream::unfold((receiver, guard), |(mut receiver, guard)| async move {
        let message = receiver.recv().await?;
        let event = Event::default()
            .id(guard.id.clone())
            .event(message.event)
            .data(message.data);
        Some((Ok::<_, Infallible>(event), (receiver, guard)))
    });
    let stream = stream::once(async move { Ok(connect) }).chain(events);

    // proxies such as nginx buffer responses unless told not to
    Ok((
        [("x-accel-buffering", "no")],
        Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(30))),
    ))
}

fn same_auth(a: &RequestAuth, b: &RequestAuth) -> bool {
    match (&a.superuser, &b.superuser, &a.record, &b.record) {
        (Some(x), Some(y), _, _) => x.id() == y.id(),
        (None, None, Some(x), Some(y)) => x.collection.id == y.collection.id && x.id() == y.id(),
        (None, None, None, None) => true,
        _ => false,
    }
}

/// `POST /api/realtime`: replaces a client's subscriptions. The request's auth becomes
/// the client's auth; a guest client may become authenticated, but a client's auth
/// can't change to another one.
pub async fn subscribe(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let client_id = body
        .get("clientId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let raw_topics: Vec<String> = match body.get("subscriptions") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<_>>()
            .ok_or_else(|| ApiError::bad_request("Subscriptions must be strings."))?,
        Some(_) => return Err(ApiError::bad_request("Subscriptions must be a list.")),
    };
    if raw_topics.len() > MAX_SUBSCRIPTIONS || raw_topics.iter().any(|t| t.len() > MAX_TOPIC_LENGTH)
    {
        return Err(ApiError::bad_request("Too many or too long subscriptions."));
    }
    let topics: Vec<Topic> = raw_topics
        .iter()
        .map(|t| {
            Topic::parse(t)
                .ok_or_else(|| ApiError::bad_request(format!("Invalid subscription {t:?}.")))
        })
        .collect::<Result<_, _>>()?;

    let client = core
        .realtime
        .get(client_id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "Missing or invalid client id."))?;
    let mut subscriptions = client.subscriptions.lock().expect("subscriptions lock");
    if let Some(previous) = &subscriptions.auth {
        if !same_auth(previous, &auth) {
            return Err(ApiError::forbidden(
                "The current and the previous request authorization don't match.",
            ));
        }
    }
    let authenticated = auth.superuser.is_some() || auth.record.is_some();
    subscriptions.auth = authenticated.then_some(auth);
    subscriptions.topics = topics;
    drop(subscriptions);

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Sends a message to a client of this instance if it's subscribed to `topic`.
pub fn send_to_client(core: &Core, client_id: &str, topic: &str, data: String) -> bool {
    let Some(client) = core.realtime.get(client_id) else {
        return false;
    };
    let subscribed = client
        .subscriptions
        .lock()
        .expect("subscriptions lock")
        .topics
        .iter()
        .any(|t| t.raw == topic);
    subscribed
        && client
            .sender
            .try_send(Message {
                event: topic.to_string(),
                data,
            })
            .is_ok()
}

// ---------------------------------------------------------------------------
// delivery

struct Target {
    client_id: String,
    client: Arc<Client>,
    auth: RequestAuth,
    topic: Topic,
}

/// Delivers a change to every subscriber allowed to see it.
pub async fn dispatch(core: &Arc<Core>, collection_id: &str, action: Action, record: Values) {
    let registry = core.registry();
    let Some(c) = registry.by_id(collection_id).cloned() else {
        return;
    };
    let record_id = record
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let mut targets = Vec::new();
    for (client_id, client) in core.realtime.snapshot() {
        let subscriptions = client.subscriptions.lock().expect("subscriptions lock");
        for topic in &subscriptions.topics {
            let matches_collection = registry
                .get(&topic.collection)
                .is_some_and(|t| t.id == c.id);
            let matches_record = topic.record_id.as_deref().is_none_or(|id| id == record_id);
            if matches_collection && matches_record {
                targets.push(Target {
                    client_id: client_id.clone(),
                    client: client.clone(),
                    auth: subscriptions.auth.clone().unwrap_or_default(),
                    topic: topic.clone(),
                });
            }
        }
    }
    if targets.is_empty() {
        return;
    }

    for chunk in targets.chunks(CHECK_CHUNK) {
        match check_access(core, &c, &record, chunk).await {
            Ok(results) => {
                for (target, access) in chunk.iter().zip(results) {
                    if let Some(email_visible) = access {
                        deliver(core, &c, action, &record, target, email_visible).await;
                    }
                }
            }
            Err(err) => {
                tracing::error!(error = %format!("{err:#}"), "realtime access check failed")
            }
        }
    }
}

/// The request a subscription's events are checked and expanded as.
fn target_request(core: &Core, target: &Target) -> RequestInfo {
    let mut request = RequestInfo::new("GET");
    request.registry = Some(core.registry());
    request.auth = target.auth.info();
    request.query = target.topic.query.clone();
    request.headers = target.topic.headers.clone();
    request
}

/// For each target: `None` if it may not see the record, else whether the email is visible.
async fn check_access(
    core: &Core,
    c: &Collection,
    record: &Values,
    targets: &[Target],
) -> anyhow::Result<Vec<Option<bool>>> {
    let mut params = Params::default();

    // the record as a one-row table aliased like a collection table
    let mut columns = Vec::new();
    for field in c.fields.iter().filter(|f| !f.is_secret()) {
        let value = record
            .get(&field.name)
            .cloned()
            .unwrap_or_else(|| values::default_value(field));
        let placeholder = match values::to_param(field, &value) {
            Some(p) => params.push(p),
            None => params.push_null(),
        };
        columns.push(format!(
            "({placeholder}){} AS {}",
            values::sql_cast(field),
            quote_ident(&field.name)
        ));
    }

    let mut selects = Vec::new();
    let mut allowed_at_all = Vec::new();
    for target in targets {
        let request = target_request(core, target);

        let action = if target.topic.record_id.is_some() {
            RuleAction::View
        } else {
            RuleAction::List
        };
        let mut conditions = Vec::new();
        let allowed = match rule_sql(c, action, &target.auth, &request, &mut params) {
            Ok(rule) => {
                conditions.extend(rule);
                true
            }
            Err(_) => false, // a null rule: superusers only
        };
        if let Some(filter) = target
            .topic
            .query
            .get("filter")
            .filter(|f| !f.trim().is_empty())
        {
            let scope = Scope {
                collection: c,
                alias: ALIAS,
                request: &request,
                allow_hidden: target.auth.is_superuser(),
            };
            match to_sql(filter, &scope, &mut params) {
                Ok(sql) => conditions.push(sql),
                Err(_) => conditions.push("FALSE".into()),
            }
        }
        let condition = if !allowed {
            "FALSE".to_string()
        } else if conditions.is_empty() {
            "TRUE".to_string()
        } else {
            conditions.join(" AND ")
        };
        let email = email_visible_sql(c, &target.auth, &request, &mut params)
            .map_err(|e| anyhow::anyhow!(e.message))?
            .unwrap_or_else(|| "TRUE".into());
        selects.push(format!("({condition})"));
        selects.push(format!("({email})"));
        allowed_at_all.push(allowed);
    }

    let sql = format!(
        "SELECT {} FROM (SELECT {}) AS {ALIAS}",
        selects.join(", "),
        columns.join(", ")
    );
    let client = core.pool.get().await.map_err(db::describe_pool_error)?;
    let row = client.query_text_one(&sql, &params).await?;

    Ok((0..targets.len())
        .map(|i| {
            let visible: Option<bool> = row.get(i * 2);
            let email: Option<bool> = row.get(i * 2 + 1);
            (allowed_at_all[i] && visible == Some(true)).then_some(email == Some(true))
        })
        .collect())
}

async fn deliver(
    core: &Arc<Core>,
    c: &Arc<Collection>,
    action: Action,
    record: &Values,
    target: &Target,
    email_visible: bool,
) {
    let fields = visible_fields(c, target.auth.is_superuser());
    let mut values = Values::new();
    for f in fields {
        if let Some(v) = record.get(&f.name) {
            values.insert(f.name.clone(), v.clone());
        }
    }
    if c.is_auth() && !email_visible {
        values.remove(crate::collections::model::FIELD_EMAIL);
    }
    let record = record_json(c, values);
    // enriched, and with the `expand` of the subscription's options (the subscriber's rules
    // apply to related records)
    let request = target_request(core, target);
    let record = match crate::records::api::respond(
        core,
        &target.auth,
        &request,
        c,
        record.clone(),
        &target.topic.query,
    )
    .await
    {
        Ok(record) => record,
        Err(err) => {
            tracing::debug!(error = %err.message, "realtime enrich or expand failed; sending the record as is");
            record
        }
    };
    let record = pick_fields(record, target.topic.query.get("fields"));
    let message = Message {
        event: target.topic.raw.clone(),
        data: json!({ "action": action.as_str(), "record": record }).to_string(),
    };
    if target.client.sender.try_send(message).is_err() {
        // a client that can't keep up (or is gone) is disconnected
        tracing::warn!(client = %target.client_id, "realtime client too slow, disconnecting");
        core.realtime.remove(&target.client_id);
    }
}

// ---------------------------------------------------------------------------
// the listener

/// Remembers recently delivered event ids, so catch-up after a reconnect doesn't
/// deliver events twice.
#[derive(Default)]
struct Seen {
    order: VecDeque<i64>,
    set: HashSet<i64>,
}

impl Seen {
    fn insert(&mut self, id: i64) -> bool {
        if !self.set.insert(id) {
            return false;
        }
        self.order.push_back(id);
        if self.order.len() > 10_000 {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }
}

async fn deliver_event(core: &Arc<Core>, id: i64, seen: &mut Seen) -> anyhow::Result<()> {
    if !seen.insert(id) {
        return Ok(());
    }
    // nobody to deliver to on this instance: skip reading the event
    if core.realtime.client_count() == 0 {
        return Ok(());
    }
    let client = core.pool.get().await.map_err(db::describe_pool_error)?;
    let Some(row) = client
        .query_opt(
            "SELECT collection_id, action, record FROM _slonbase.events WHERE id = $1",
            &[&id],
        )
        .await?
    else {
        return Ok(());
    };
    drop(client);
    let collection_id: String = row.get(0);
    let action: String = row.get(1);
    let record: Value = row.get(2);
    if let (Some(action), Value::Object(record)) = (Action::parse(&action), record) {
        if core.realtime.client_count() > 0 {
            dispatch(core, &collection_id, action, record).await;
        }
    }
    Ok(())
}

async fn reload_collections(core: &Core) {
    // A change made here while loading replaces the registry with one it read after its
    // own commit; this load may predate that, so it's discarded and read again (it may
    // also have been the only one to see another instance's change).
    for _ in 0..5 {
        let generation = core.registry_generation();
        match store::load_from_pool(&core.pool).await {
            Ok(registry) => {
                if core.set_registry_if(generation, registry) {
                    return;
                }
            }
            Err(err) => {
                tracing::error!(error = %format!("{err:#}"), "failed to reload collections");
                return;
            }
        }
    }
}

async fn reload_settings(core: &Core) {
    // as for collections: don't undo a newer change made here
    for _ in 0..5 {
        let generation = core.settings_generation();
        let loaded = async {
            let client = core.pool.get().await.map_err(db::describe_pool_error)?;
            Settings::load(&client).await
        };
        match loaded.await {
            Ok(settings) => {
                if core.set_settings_if(generation, settings) {
                    return;
                }
            }
            Err(err) => {
                tracing::error!(error = %format!("{err:#}"), "failed to reload settings");
                return;
            }
        }
    }
}

/// Runs the listener until the core is dropped: delivers events, reloads collections and
/// settings changed by other instances, prunes old events, and reconnects with backoff
/// (catching up on events and changes it may have missed).
pub fn spawn_listener(core: &Arc<Core>, url: String, cfg: DatabaseConfig) {
    let weak = Arc::downgrade(core);
    tokio::spawn(async move {
        let mut seen = Seen::default();
        let mut delay = Duration::from_secs(1);
        let mut disconnected_at: Option<chrono::DateTime<chrono::Utc>> = None;

        loop {
            if weak.strong_count() == 0 {
                return;
            }
            let (_client, mut notifications) = match db::listen(&url, &cfg, CHANNEL).await {
                Ok(connection) => connection,
                Err(err) => {
                    tracing::warn!(error = %format!("{err:#}"), retry_in = ?delay, "realtime listener failed to connect");
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(30));
                    continue;
                }
            };
            delay = Duration::from_secs(1);

            // catch up on what happened while disconnected
            if let (Some(since), Some(core)) = (disconnected_at.take(), weak.upgrade()) {
                tracing::info!("realtime listener reconnected, catching up");
                reload_collections(&core).await;
                reload_settings(&core).await;
                let missed = async {
                    let client = core.pool.get().await.map_err(db::describe_pool_error)?;
                    let rows = client
                        .query(
                            "SELECT id FROM _slonbase.events WHERE created >= $1::text::timestamptz - interval '5 seconds' ORDER BY id",
                            &[&since.to_rfc3339()],
                        )
                        .await?;
                    anyhow::Ok(rows.iter().map(|r| r.get::<_, i64>(0)).collect::<Vec<_>>())
                };
                match missed.await {
                    Ok(ids) => {
                        for id in ids {
                            if let Err(err) = deliver_event(&core, id, &mut seen).await {
                                tracing::error!(error = %format!("{err:#}"), "failed to deliver an event");
                            }
                        }
                    }
                    Err(err) => {
                        tracing::error!(error = %format!("{err:#}"), "failed to catch up on events")
                    }
                }
            }

            let mut prune = tokio::time::interval(Duration::from_secs(60));
            // more often than once a minute, so no scheduled minute is skipped
            let mut schedule = tokio::time::interval(Duration::from_secs(20));
            loop {
                tokio::select! {
                    notification = notifications.recv() => {
                        let Some(notification) = notification else { break };
                        let Some(core) = weak.upgrade() else { return };
                        let payload = notification.payload();
                        if payload == "collections" {
                            reload_collections(&core).await;
                        } else if payload == "settings" {
                            reload_settings(&core).await;
                        } else if payload == "restored" {
                            // another instance restored a backup
                            if let Err(err) = core.reload_after_restore().await {
                                tracing::error!(error = %format!("{err:#}"), "failed to reload after a restore");
                            }
                        } else if let Some(data) = payload.strip_prefix("oauth2:") {
                            let state = serde_json::from_str::<Value>(data)
                                .ok()
                                .and_then(|v| v["state"].as_str().map(str::to_string));
                            if let Some(state) = state {
                                send_to_client(&core, &state, OAUTH2_TOPIC, data.to_string());
                            }
                        } else if let Some(id) = payload.strip_prefix("event:").and_then(|s| s.parse().ok()) {
                            if let Err(err) = deliver_event(&core, id, &mut seen).await {
                                tracing::error!(error = %format!("{err:#}"), "failed to deliver an event");
                            }
                        }
                    }
                    _ = schedule.tick() => {
                        let Some(core) = weak.upgrade() else { return };
                        crate::backups::run_scheduled(&core).await;
                        crate::hooks::run_scheduled_crons(&core).await;
                    }
                    _ = prune.tick() => {
                        let Some(core) = weak.upgrade() else { return };
                        core.rate_limiter.prune();
                        let max_days = core.settings().logs.max_days;
                        if max_days > 0 {
                            if let Err(err) = crate::logs::cleanup(&core.pool, max_days).await {
                                tracing::debug!(error = %format!("{err:#}"), "failed to delete old logs");
                            }
                        }
                        if let Ok(client) = core.pool.get().await {
                            let _ = client
                                .batch_execute(&format!(
                                    "DELETE FROM _slonbase.events WHERE created < now() - interval '{EVENT_RETENTION_SECS} seconds';
                                     DELETE FROM _slonbase.otps WHERE created < now() - interval '{AUTH_RETENTION_SECS} seconds';
                                     DELETE FROM _slonbase.mfas WHERE created < now() - interval '{AUTH_RETENTION_SECS} seconds';"
                                ))
                                .await;
                        }
                    }
                }
            }
            disconnected_at = Some(chrono::Utc::now());
            tracing::warn!("realtime listener disconnected, reconnecting");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_topics() {
        let t = Topic::parse("posts/*").unwrap();
        assert_eq!(
            (t.collection.as_str(), t.record_id.as_deref()),
            ("posts", None)
        );
        let t = Topic::parse("posts").unwrap();
        assert_eq!(t.record_id, None);
        let t = Topic::parse("posts/abc123").unwrap();
        assert_eq!(t.record_id.as_deref(), Some("abc123"));

        let raw = format!(
            "posts/*?options={}",
            form_urlencoded::byte_serialize(
                br#"{"query":{"filter":"a = 1","n":5},"headers":{"X-Test":"y"}}"#
            )
            .collect::<String>()
        );
        let t = Topic::parse(&raw).unwrap();
        assert_eq!(t.raw, raw);
        assert_eq!(t.query.get("filter").map(String::as_str), Some("a = 1"));
        assert_eq!(t.query.get("n").map(String::as_str), Some("5"));
        assert_eq!(t.headers.get("x_test").map(String::as_str), Some("y"));

        for bad in ["", "/x", "posts/", "posts/a/b", "posts?options=notjson"] {
            assert!(Topic::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn seen_ids() {
        let mut seen = Seen::default();
        assert!(seen.insert(1));
        assert!(!seen.insert(1));
        for i in 2..20_000 {
            seen.insert(i);
        }
        assert!(seen.set.len() <= 10_000);
        assert!(seen.insert(1), "old ids are forgotten");
    }
}

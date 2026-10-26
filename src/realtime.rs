//! Realtime: record change events delivered over Server-Sent Events, compatible with
//! PocketBase's `/api/realtime` protocol and SDKs.
//!
//! Record changes are written to an outbox (`_slonbase.events`) and announced with
//! `pg_notify` by a trigger on each collection table (`migrations/0010_realtime_triggers.sql`),
//! in the transaction of the change, so changes made outside the API (SQL, other programs)
//! are delivered too, and rolled-back changes never are. Every
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

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::http::{HeaderMap, Method};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use deadpool_postgres::GenericClient;
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
use crate::hooks::{self, DataEvent};
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

pub struct Hub {
    clients: Mutex<HashMap<String, Arc<Client>>>,
    /// This instance's id in `_slonbase.instances`.
    instance: String,
    /// Subscribe requests forwarded to the other instances, waiting for their answers: `true`
    /// from the one holding the client, `false` from each one that doesn't.
    forwards: Mutex<HashMap<i64, mpsc::UnboundedSender<bool>>>,
    /// The other live instances, as of the last heartbeat.
    others: std::sync::atomic::AtomicUsize,
    /// Whether this instance told the others it started (so they count it right away).
    announced: std::sync::atomic::AtomicBool,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            clients: Mutex::default(),
            instance: ids::random_id(15),
            forwards: Mutex::default(),
            others: Default::default(),
            announced: Default::default(),
        }
    }
}

impl Hub {
    fn register(&self, id: &str) -> mpsc::Receiver<Message> {
        let (sender, receiver) = mpsc::channel(CLIENT_BUFFER);
        self.clients.lock().expect("hub lock").insert(
            id.to_string(),
            Arc::new(Client {
                sender,
                subscriptions: Mutex::new(Subscriptions::default()),
            }),
        );
        receiver
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

    /// How many other instances share the database (as of the last heartbeat, at most 20
    /// seconds old).
    pub fn other_instances(&self) -> usize {
        self.others.load(std::sync::atomic::Ordering::Relaxed)
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
pub async fn connect(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let id = ids::random_id(40);
    let request = Arc::new(crate::records::api::request_info(
        &Method::GET,
        &query,
        &headers,
        None,
        &auth,
        &core.registry(),
    ));
    let opened: Arc<Mutex<Option<mpsc::Receiver<Message>>>> = Default::default();
    let e = DataEvent {
        core: core.clone(),
        collection: None,
        auth,
        request: Some(request.clone()),
        data: hooks::data([
            ("client", client_json(&id, &[], None)),
            ("idleTimeout", 0.into()),
            ("maxTimeout", 0.into()),
        ]),
        response: None,
    };
    let result = {
        let (opened, core, id) = (opened.clone(), core.clone(), id.clone());
        core.hooks
            .clone()
            .on_realtime_connect_request
            .trigger(None, e, move |e| async move {
                *opened.lock().expect("connect lock") = Some(core.realtime.register(&id));
                Ok(e)
            })
            .await
    };
    let receiver = opened.lock().expect("connect lock").take();
    let e = match result {
        Ok(e) => e,
        Err(err) => {
            core.realtime.remove(&id);
            return Err(err);
        }
    };
    let Some(receiver) = receiver else {
        // refused by a handler
        return Ok(e.response.unwrap_or_else(|| StatusCode::OK.into_response()));
    };

    let state = StreamState {
        first: Some(Message {
            event: "PB_CONNECT".into(),
            data: json!({ "clientId": id }).to_string(),
        }),
        receiver,
        guard: ClientGuard {
            hub: Arc::downgrade(&core),
            id,
        },
        request,
        idle: nanos(&e.data["idleTimeout"]),
        deadline: nanos(&e.data["maxTimeout"]).map(|d| tokio::time::Instant::now() + d),
    };
    let stream = stream::unfold(state, |mut state| async move {
        loop {
            let message = match state.first.take() {
                Some(first) => first,
                None => state.recv().await?,
            };
            let core = state.guard.hub.upgrade()?;
            match message_hook(&core, &state.guard.id, &state.request, message).await {
                Ok(Some(message)) => {
                    let event = Event::default()
                        .id(state.guard.id.clone())
                        .event(message.event)
                        .data(message.data);
                    return Some((Ok::<_, Infallible>(event), state));
                }
                Ok(None) => continue,
                Err(err) => {
                    tracing::debug!(client = %state.guard.id, error = %err.message, "a realtime message hook closed the connection");
                    return None;
                }
            }
        }
    });

    // proxies such as nginx buffer responses unless told not to
    Ok((
        [("x-accel-buffering", "no")],
        Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(30))),
    )
        .into_response())
}

/// The state of a client's event stream.
struct StreamState {
    /// `PB_CONNECT`, until it's sent.
    first: Option<Message>,
    receiver: mpsc::Receiver<Message>,
    guard: ClientGuard,
    /// The connect request (for the message hook).
    request: Arc<RequestInfo>,
    /// Closes the stream after this long without a message.
    idle: Option<Duration>,
    /// Closes the stream at this time.
    deadline: Option<tokio::time::Instant>,
}

impl StreamState {
    /// The next message, or `None` when the client is gone or a timeout passed.
    async fn recv(&mut self) -> Option<Message> {
        let idle_until = self.idle.map(|d| tokio::time::Instant::now() + d);
        let until = match (idle_until, self.deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        match until {
            None => self.receiver.recv().await,
            Some(until) => tokio::time::timeout_at(until, self.receiver.recv())
                .await
                .ok()
                .flatten(),
        }
    }
}

/// A duration given in nanoseconds (like Go's), `None` for zero or none.
fn nanos(value: &Value) -> Option<Duration> {
    value
        .as_f64()
        .filter(|n| *n >= 1.0)
        .map(|n| Duration::from_nanos(n as u64))
}

/// A client as hooks see it: its id, subscriptions and auth record (without secrets).
fn client_json(id: &str, subscriptions: &[String], auth: Option<&RequestAuth>) -> Value {
    let auth = auth
        .and_then(RequestAuth::any_record)
        .map(|record| {
            let mut values = record.values.clone();
            for field in record.collection.fields.iter().filter(|f| f.is_secret()) {
                values.remove(&field.name);
            }
            crate::records::store::record_json(&record.collection, values)
        })
        .unwrap_or(Value::Null);
    json!({ "id": id, "subscriptions": subscriptions, "auth": auth })
}

/// The client's subscriptions and auth, as [`client_json`].
fn client_state(core: &Core, id: &str) -> (Value, RequestAuth) {
    let Some(client) = core.realtime.get(id) else {
        return (client_json(id, &[], None), RequestAuth::default());
    };
    let subscriptions = client.subscriptions.lock().expect("subscriptions lock");
    let topics: Vec<String> = subscriptions.topics.iter().map(|t| t.raw.clone()).collect();
    let auth = subscriptions.auth.clone().unwrap_or_default();
    (client_json(id, &topics, Some(&auth)), auth)
}

/// Runs `on_realtime_message_send` for a message: the message to send (as the handlers
/// left it), `None` to skip it, or an error that closes the connection.
async fn message_hook(
    core: &Arc<Core>,
    client_id: &str,
    request: &Arc<RequestInfo>,
    message: Message,
) -> Result<Option<Message>, ApiError> {
    let hook = &core.hooks.on_realtime_message_send;
    if !hook.has_handlers(None) {
        return Ok(Some(message));
    }
    let (client, auth) = client_state(core, client_id);
    let e = DataEvent {
        core: core.clone(),
        collection: None,
        auth,
        request: Some(request.clone()),
        data: hooks::data([
            ("client", client),
            (
                "message",
                json!({ "name": message.event, "data": message.data }),
            ),
        ]),
        response: None,
    };
    let sent = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let e = {
        let sent = sent.clone();
        hook.trigger(None, e, move |e| async move {
            sent.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(e)
        })
        .await?
    };
    if !sent.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(None);
    }
    let out = &e.data["message"];
    let event = out["name"].as_str().unwrap_or(&message.event).to_string();
    let data = match &out["data"] {
        Value::String(s) => s.clone(),
        Value::Null => message.data,
        // bytes (PocketBase's []byte)
        Value::Array(items) if items.iter().all(Value::is_u64) => String::from_utf8_lossy(
            &items
                .iter()
                .filter_map(Value::as_u64)
                .map(|b| b as u8)
                .collect::<Vec<u8>>(),
        )
        .into_owned(),
        other => other.to_string(),
    };
    Ok(Some(Message { event, data }))
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
/// can't change to another one. A client whose stream is on another instance is handled
/// there (see [`forward`]), so no client affinity is needed.
pub async fn subscribe(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    headers: HeaderMap,
    JsonObject(body): JsonObject,
) -> Result<Response, ApiError> {
    let core = app.core()?;
    let client_id = body
        .get("clientId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if core.realtime.get(client_id).is_none() {
        if let Some(response) = forward(&core, client_id, &headers, &body).await? {
            return Ok(response);
        }
    }
    subscribe_here(&core, auth, &headers, body).await
}

/// How long a forwarded subscribe request waits for the instance holding the client.
const FORWARD_TIMEOUT: Duration = Duration::from_secs(5);

/// Forwards a subscribe request for a client this instance doesn't hold to the other live
/// instances, through the database: the request goes to `_slonbase.realtime_forwards` and is
/// announced; the instance holding the client runs it (authenticating its `Authorization`
/// itself) and writes the response back. `None` when no instance holds the client.
async fn forward(
    core: &Arc<Core>,
    client_id: &str,
    headers: &HeaderMap,
    body: &serde_json::Map<String, Value>,
) -> Result<Option<Response>, ApiError> {
    if client_id.is_empty() {
        return Ok(None);
    }
    let pool_error = |e| ApiError::internal(anyhow::anyhow!(db::describe_pool_error(e)));
    let db_error = |e: tokio_postgres::Error| ApiError::internal(e.into());
    let hub = &core.realtime;
    let headers: serde_json::Map<String, Value> = headers
        .iter()
        .filter_map(|(k, v)| {
            Some((
                k.as_str().to_string(),
                Value::String(v.to_str().ok()?.to_string()),
            ))
        })
        .collect();
    let request = json!({ "headers": headers, "body": body });
    let (id, others) = {
        let client = core.pool.get().await.map_err(pool_error)?;
        let others: i64 = client
            .query_one(
                "SELECT count(*) FROM _slonbase.instances WHERE id <> $1 AND seen > now() - interval '60 seconds'",
                &[&hub.instance],
            )
            .await
            .map_err(db_error)?
            .get(0);
        if others == 0 {
            return Ok(None);
        }
        let id: i64 = client
            .query_one(
                "INSERT INTO _slonbase.realtime_forwards (client_id, request) VALUES ($1, $2) RETURNING id",
                &[&client_id, &request],
            )
            .await
            .map_err(db_error)?
            .get(0);
        (id, others)
    };
    let (sender, mut answers) = mpsc::unbounded_channel();
    hub.forwards
        .lock()
        .expect("forwards lock")
        .insert(id, sender);
    let announced = notify_with(core, &format!("forward:{id}:{}:{client_id}", hub.instance)).await;
    let held = match announced {
        Ok(()) => tokio::time::timeout(FORWARD_TIMEOUT, async {
            let mut missing = 0;
            while let Some(held) = answers.recv().await {
                if held {
                    return true;
                }
                missing += 1;
                if missing >= others {
                    break;
                }
            }
            false
        })
        .await
        .unwrap_or(false),
        Err(_) => false,
    };
    hub.forwards.lock().expect("forwards lock").remove(&id);
    let client = core.pool.get().await.map_err(pool_error)?;
    let response: Option<Value> = client
        .query_opt(
            "DELETE FROM _slonbase.realtime_forwards WHERE id = $1 RETURNING response",
            &[&id],
        )
        .await
        .map_err(db_error)?
        .and_then(|row| row.get(0));
    Ok(match (held, response) {
        (true, Some(response)) => Some(stored_response(&response)),
        _ => None,
    })
}

async fn notify_with(core: &Core, payload: &str) -> anyhow::Result<()> {
    let client = core.pool.get().await.map_err(db::describe_pool_error)?;
    notify(&client, payload).await?;
    Ok(())
}

fn stored_response(response: &Value) -> Response {
    let status = response["status"]
        .as_u64()
        .and_then(|s| StatusCode::from_u16(s as u16).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = response["contentType"].as_str() {
        builder = builder.header(axum::http::header::CONTENT_TYPE, content_type);
    }
    let body = response["body"].as_str().unwrap_or_default().to_string();
    builder
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Another instance's forwarded subscribe request (`forward:{id}:{instance}:{client}`): run it
/// here if this instance holds the client, else say so.
fn handle_forward(core: &Arc<Core>, payload: &str) {
    let mut parts = payload.splitn(3, ':');
    let (Some(Ok(id)), Some(instance), Some(client_id)) = (
        parts.next().map(str::parse::<i64>),
        parts.next(),
        parts.next(),
    ) else {
        return;
    };
    if instance == core.realtime.instance {
        return;
    }
    let (core, client_id) = (core.clone(), client_id.to_string());
    tokio::spawn(async move {
        let held = core.realtime.get(&client_id).is_some();
        if held {
            if let Err(err) = run_forwarded(&core, id).await {
                tracing::error!(error = %format!("{err:#}"), "failed to run a forwarded realtime request");
            }
        }
        if let Err(err) = notify_with(&core, &format!("forwarded:{id}:{}", u8::from(held))).await {
            tracing::error!(error = %format!("{err:#}"), "failed to answer a forwarded realtime request");
        }
    });
}

async fn run_forwarded(core: &Arc<Core>, id: i64) -> anyhow::Result<()> {
    let client = core.pool.get().await.map_err(db::describe_pool_error)?;
    let Some(row) = client
        .query_opt(
            "SELECT request FROM _slonbase.realtime_forwards WHERE id = $1",
            &[&id],
        )
        .await?
    else {
        return Ok(());
    };
    drop(client);
    let request: Value = row.get(0);
    let mut headers = HeaderMap::new();
    for (name, value) in request["headers"].as_object().into_iter().flatten() {
        if let (Ok(name), Some(Ok(value))) = (
            axum::http::HeaderName::from_bytes(name.as_bytes()),
            value.as_str().map(axum::http::HeaderValue::from_str),
        ) {
            headers.insert(name, value);
        }
    }
    // the original request's auth, checked here
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(crate::auth::token_from_header);
    let auth = match token {
        Some(token) => crate::auth::authenticate(core, token).await,
        None => RequestAuth::default(),
    };
    let body = request["body"].as_object().cloned().unwrap_or_default();
    let response = match subscribe_here(core, auth, &headers, body).await {
        Ok(response) => response,
        Err(err) => err.into_response(),
    };
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await?;
    let stored = json!({
        "status": status,
        "contentType": content_type,
        "body": String::from_utf8_lossy(&bytes),
    });
    let client = core.pool.get().await.map_err(db::describe_pool_error)?;
    client
        .execute(
            "UPDATE _slonbase.realtime_forwards SET response = $2 WHERE id = $1",
            &[&id, &stored],
        )
        .await?;
    Ok(())
}

/// [`subscribe`] for a client of this instance.
async fn subscribe_here(
    core: &Arc<Core>,
    auth: RequestAuth,
    headers: &HeaderMap,
    body: serde_json::Map<String, Value>,
) -> Result<Response, ApiError> {
    let core = core.clone();
    let client_id = body
        .get("clientId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let subscriptions = body.get("subscriptions").cloned().unwrap_or(Value::Null);
    // invalid subscriptions are refused before the hooks, like PocketBase
    parse_topics(&subscriptions)?;
    let raw_topics = match subscriptions {
        Value::Null => Value::Array(vec![]),
        list => list,
    };

    let client = core
        .realtime
        .get(client_id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "Missing or invalid client id."))?;
    {
        let subscriptions = client.subscriptions.lock().expect("subscriptions lock");
        if let Some(previous) = &subscriptions.auth {
            if !same_auth(previous, &auth) {
                return Err(ApiError::forbidden(
                    "The current and the previous request authorization don't match.",
                ));
            }
        }
    }

    let (client_value, _) = client_state(&core, client_id);
    let request = crate::records::api::request_info(
        &Method::POST,
        &HashMap::new(),
        headers,
        Some(&body),
        &auth,
        &core.registry(),
    );
    let e = DataEvent {
        core: core.clone(),
        collection: None,
        auth: auth.clone(),
        request: Some(Arc::new(request)),
        data: hooks::data([("client", client_value), ("subscriptions", raw_topics)]),
        response: None,
    };
    let applied = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let e = {
        let applied = applied.clone();
        core.hooks
            .on_realtime_subscribe_request
            .trigger(None, e, move |e| async move {
                let topics = parse_topics(&e.data["subscriptions"])?;
                let mut subscriptions = client.subscriptions.lock().expect("subscriptions lock");
                let authenticated = e.auth.superuser.is_some() || e.auth.record.is_some();
                subscriptions.auth = authenticated.then(|| e.auth.clone());
                subscriptions.topics = topics;
                drop(subscriptions);
                applied.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(e)
            })
            .await?
    };
    if let Some(response) = e.response {
        return Ok(response);
    }
    Ok(if applied.load(std::sync::atomic::Ordering::SeqCst) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::OK.into_response()
    })
}

/// Subscriptions (a list of topic strings) as topics, within the limits.
fn parse_topics(value: &Value) -> Result<Vec<Topic>, ApiError> {
    let raw: Vec<String> = match value {
        Value::Null => vec![],
        Value::Array(items) => items
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<_>>()
            .ok_or_else(|| ApiError::bad_request("Subscriptions must be strings."))?,
        _ => return Err(ApiError::bad_request("Subscriptions must be a list.")),
    };
    if raw.len() > MAX_SUBSCRIPTIONS || raw.iter().any(|t| t.len() > MAX_TOPIC_LENGTH) {
        return Err(ApiError::bad_request("Too many or too long subscriptions."));
    }
    raw.iter()
        .map(|t| {
            Topic::parse(t)
                .ok_or_else(|| ApiError::bad_request(format!("Invalid subscription {t:?}.")))
        })
        .collect()
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
        crate::metrics::METRICS
            .realtime_events
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

async fn reload_settings(core: &Arc<Core>) {
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
                    crate::settings::reloaded(core);
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

/// Marks this instance as live (every 20 seconds, from the listener), so other instances
/// forward subscribe requests to it, and counts the others (for shared rate limits).
async fn heartbeat(core: &Core) {
    let beat = async {
        let client = core.pool.get().await.map_err(db::describe_pool_error)?;
        let others: i64 = client
            .query_one(
                "WITH beat AS (INSERT INTO _slonbase.instances (id) VALUES ($1) \
                     ON CONFLICT (id) DO UPDATE SET seen = now()) \
                 SELECT count(*) FROM _slonbase.instances WHERE id <> $1 AND seen > now() - interval '60 seconds'",
                &[&core.realtime.instance],
            )
            .await?
            .get(0);
        core.realtime
            .others
            .store(others as usize, std::sync::atomic::Ordering::Relaxed);
        if !core
            .realtime
            .announced
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            notify(&client, "instance").await?;
        }
        anyhow::Ok(())
    };
    if let Err(err) = beat.await {
        tracing::debug!(error = %format!("{err:#}"), "failed to record the instance heartbeat");
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
                        } else if payload == "instance" {
                            // an instance started: count it
                            heartbeat(&core).await;
                        } else if let Some(rest) = payload.strip_prefix("forward:") {
                            handle_forward(&core, rest);
                        } else if let Some(rest) = payload.strip_prefix("forwarded:") {
                            if let Some((id, held)) = rest.split_once(':') {
                                let waiting = id.parse().ok().and_then(|id: i64| {
                                    core.realtime.forwards.lock().expect("forwards lock").get(&id).cloned()
                                });
                                if let Some(waiting) = waiting {
                                    let _ = waiting.send(held == "1");
                                }
                            }
                        } else if let Some(id) = payload.strip_prefix("event:").and_then(|s| s.parse().ok()) {
                            if let Err(err) = deliver_event(&core, id, &mut seen).await {
                                tracing::error!(error = %format!("{err:#}"), "failed to deliver an event");
                            }
                        }
                    }
                    _ = schedule.tick() => {
                        let Some(core) = weak.upgrade() else { return };
                        heartbeat(&core).await;
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
                                     DELETE FROM _slonbase.instances WHERE seen < now() - interval '10 minutes';
                                     DELETE FROM _slonbase.realtime_forwards WHERE created < now() - interval '1 minute';
                                     DELETE FROM _slonbase.rate_limits WHERE started < now() - interval '1 hour';
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

# slonbase design

This document records what slonbase is, the decisions behind it, and the plan. Update it when a
decision changes.

## Goals

- **A complete backend on PostgreSQL**, with the quick, all-in-one developer experience
  PocketBase made popular: collections defined at runtime, an auto-generated REST API, filtering
  and access rules, auth, realtime, files, hooks, an admin dashboard. Running on PostgreSQL makes
  the data ordinary tables and lets several instances share it.
- **A single small binary** (currently ~18 MB static, an 8 MB download) with low memory use and
  predictable latency.
- **Rock-solid operation**: safe startup ordering, safe concurrent instances, no data-loss paths,
  clear errors.
- **Easy to install**: Docker Compose, `.deb`/`.rpm` packages, or a plain binary.

## Non-goals

- Bundling PostgreSQL. The database is always external; slonbase connects to it.
- Supporting databases other than PostgreSQL.

## Why a new project instead of a PocketBase fork

slonbase is its own codebase, written from scratch in Rust. PocketBase inspired it, and its API
stays compatible with PocketBase's SDKs, but no PocketBase code is used.


PocketBase's SQL is written for SQLite throughout (JSON functions, `PRAGMA`s, `sqlite_master`,
`_rowid_`, type affinity), and upstream changes those files constantly (for example
`core/base.go` changed in 41 commits between mid-2024 and late 2026). A fork that edits them
falls behind quickly. [thewandererbg/pgbase](https://github.com/thewandererbg/pgbase), a direct
PocketBase-on-PostgreSQL fork, is an example: it is still based on PocketBase 0.27 while upstream
is at 0.40.

Building fresh lets the design use PostgreSQL properly instead of emulating SQLite.

## Language and stack

**Rust**, as a single language: no garbage-collector pauses, low memory, strong compile-time
guarantees, and small static binaries.

| Concern | Choice |
|---|---|
| HTTP | `axum` on `tokio`, `tower` middleware |
| PostgreSQL | `tokio-postgres` + `deadpool-postgres` (tables are defined at runtime, so `sqlx`'s compile-time checks wouldn't help) |
| TLS | `rustls` with Mozilla roots (`webpki-roots`), plus an optional CA file |
| Passwords | `bcrypt` (cost 12), compatible with PocketBase hashes for migrations |
| Config | TOML file + env vars + flags (`clap`) |

Later phases added `image` (thumbnails), `lettre` (mail), `reqwest` (OAuth2 and a small
built-in S3 client) and an embedded JavaScript engine (`rquickjs`, QuickJS) for hooks and
migrations.

## SDK compatibility

slonbase's REST API keeps the routes, filter syntax and response and error shapes of PocketBase's,
so apps built with PocketBase's client SDKs work unchanged, and slonbase can be tested by sending
the same requests to both servers and comparing responses (the [compat suite](../compat/)). Errors already use
PocketBase's `{"status", "message", "data"}` shape and `/api/health` its `{"code", "message",
"data"}` shape.

## Architecture

### Startup state machine

```text
Starting ──(no database configured, or no superuser yet)──▶ Setup ──(setup completed)──▶ Ready
    └──────────────────────(installed)──────────────────────────────────────────────────▲
```

- **Starting**: the HTTP server is already listening (health checks answer `503`) while the
  database is checked and migrated. Unreachable databases are retried with backoff (1s up to
  30s), so slonbase can start before its database.
- **Setup**: only the setup wizard and its API are available, behind a one-time token.
- **Ready**: normal operation. Never returns to `Setup`; a database outage makes requests fail
  with `503` while the pool reconnects. Falling back to setup would let an attacker who can
  break the database connection take the server over.

### First-run setup security

- The setup token (256 bits, random) is printed to the logs or journal. Every setup API call
  requires it in the `X-Setup-Token` header, compared in constant time. The wizard strips it
  from the address bar after loading.
- Setup completion is serialized by a lock, and the first superuser is created inside a
  transaction with an advisory lock, so racing requests or instances can't create two.
- Connecting to a database that already contains an installation requires signing in as one of
  its superusers, so a new server can't silently attach to someone else's data.
- When the database URL comes from a flag or env var, the wizard can't replace it, and nothing
  is written to the config file.
- The config file is written atomically (temp file, fsync, rename) with `0600` permissions,
  since it contains the database password.

### Database layout

- System tables live in the **`_slonbase` schema**, so slonbase can share a database with other
  applications. Startup refuses a `_slonbase` schema it didn't create.
- User collections are **real typed tables** in their own schema: `jsonb`,
  `timestamptz`, `boolean`, numeric types, with GIN indexes for JSON. PostgreSQL's transactional
  DDL means a failed schema change rolls back completely.
- Record ids keep PocketBase's 15-character `[a-z0-9]` format. Unlike pgbase (which sorts and
  counts by `ctid`, a physical row address that changes on updates), default ordering will use a
  stable column.
- Emails are compared case-insensitively through `lower(email)` indexes. pgbase's ICU collation
  at level 1 also ignores accents, which would make `josé@` and `jose@` collide.

### Migrations

All pending system migrations run in **one transaction** holding a transaction-level advisory
lock (`pg_advisory_xact_lock`). Several instances starting at once apply them exactly once, and
a failed migration leaves the database unchanged. A database migrated by a newer slonbase is
refused, so an accidental downgrade can't damage it.

### Phase 1: collections and records

**Storage.** Collection definitions are stored as PocketBase-format JSON in
`_slonbase.collections`. Each collection is a table in the `slonbase` schema with one column per
field, plus a hidden `_seq` identity column (indexed) that gives a stable insertion order for
default sorting and pagination:

| Field | Column |
|---|---|
| `text`, `editor`, `email`, `url`, single `select`/`relation` | `TEXT NOT NULL DEFAULT ''` |
| `number` | `DOUBLE PRECISION NOT NULL DEFAULT 0` |
| `bool` | `BOOLEAN NOT NULL DEFAULT FALSE` |
| `date`, `autodate` | `TIMESTAMPTZ` (NULL when empty, returned as `""`) |
| multiple `select`/`relation` | `JSONB NOT NULL DEFAULT '[]'` (array of strings) |
| `json` | `JSONB` |
| `id` | `TEXT PRIMARY KEY` |

**Schema changes** run the DDL and the metadata update in one transaction under the schema
advisory lock. Fields are matched by id, so renames keep the data. Renames go through temporary
names, so two fields can swap names. Switching a select or relation between single and multiple
values converts the column in place. Changing a field's type is rejected, as in PocketBase.
Before validating a change, the registry is reloaded from the database, so changes made by
other instances are seen.

**Indexes** are accepted as PocketBase `CREATE INDEX` strings but parsed into a strict subset
(plain column lists). The SQL that runs is regenerated with quoted identifiers, never passed
through. Expressions, `COLLATE` and partial indexes are rejected for now.

**Filters and rules** are parsed once into an AST and compiled per request into parameterized
SQL. All values are bound as `text` parameters with explicit casts, never inlined. SQLite (and so
PocketBase) compares mixed types loosely, but PostgreSQL rejects mixed-type comparisons, so the
compiler gives each comparison one type:
- A literal is converted to the column's type when possible: `views > '10'` compares numbers,
  `created > '2024-01-01'` compares timestamps.
- Otherwise both sides are compared as text.
- JSON values compare as numbers through `_slonbase.to_num()`, which returns NULL instead of
  failing on non-numbers.
- Text ordering uses `COLLATE "C"`, matching SQLite's byte order.
- Multi-valued fields follow PocketBase: plain operators must match all items, `?` operators any
  item, and an empty list behaves like `[""]`.

A test runs every combination of field type, literal kind and operator (over 12,000 filters)
against PostgreSQL and requires that none fails with a database error.

**Rules.** `null` means superusers only (403), `""` anyone, anything else a filter:
- List requests apply it as an extra condition.
- View, update and delete return 404 when the rule hides the record.
- Create inserts the record, checks the rule against it in the same transaction, and rolls back
  with 400 on failure.
- Superusers bypass rules and see hidden fields. Others can't filter or sort by hidden fields.

**Writes** run in transactions:
- Updates lock the row (`FOR UPDATE`).
- Relation ids are checked to exist.
- Unique index violations become `Value must be unique.` field errors.

Deleting a record handles the relation fields pointing to it:
- `cascadeDelete` fields delete the referencing records (recursively, with cycle protection).
- Optional fields drop the id.
- Required fields that would become empty block the delete.

**Superuser auth.** HS256 JWTs signed with the installation secret (generated once in
`_slonbase.params`) plus the user's `token_key`, valid for one day. Invalid tokens make the
request a guest request, as in PocketBase.

**Differences from PocketBase so far:**
- Field names can't start with `_` (reserved for internal columns).
- `autogeneratePattern` takes any regular expression, as in PocketBase, and values are random
  strings it matches (`[charset]{n}`, the common form, has a fast path). As in PocketBase,
  anchors and word boundaries are refused, unbounded repetitions stop at 6, `.` gives letters
  and digits, and a pattern is tried ten times against the field's own limits when the
  collection is saved. Other large classes (`[^...]`, `\W`) give printable ASCII characters
  from the class, where PocketBase can give any character, control characters included.

Relation paths, `expand`, `@collection`, auth and view collections, file fields and multipart
bodies, all missing in this phase, came in later phases (below).

### Phase 2: auth

**Auth collections** (`"type": "auth"`) get PocketBase's system fields:
- `password`: a bcrypt hash, never returned.
- `tokenKey`: a random 50-character key, never returned.
- `email`, `emailVisibility`, `verified`.

They also get its auth options: `authRule`, `manageRule`, `passwordAuth`, token durations and
email templates, plus `oauth2`, `otp` and `mfa` (see below). The auth alert option is accepted
and stored for compatibility but not implemented yet. A collection's type can't change, and
system fields can't be removed or renamed.

**Indexes.** PocketBase's email index is partial (`WHERE email != ''`), which slonbase's safe index
subset doesn't allow, so auth collections get two system-managed unique indexes that aren't listed
in `indexes`:
- `lower(email) WHERE email <> ''` (case-insensitive, empty emails allowed)
- `tokenKey`

**Identity fields** other than `email` must have a unique index, so a login can't match several
records.

**Tokens** are HS256 JWTs. The signing key is derived from:
- the installation secret
- the collection id
- the token type (`auth`, `verification`, `passwordReset`, `emailChange`)
- the record's `tokenKey`

A verification token can't be used as an auth token. Changing the password, resetting it, or
changing the email rotates `tokenKey`, which signs out every session and makes reset and email
change tokens single-use. Impersonation tokens (superusers only) aren't refreshable.

**Records API for auth collections:**
- Creating a record requires `password` and `passwordConfirm`.
- Only superusers and managers can change `verified` or `email` directly. Others must use the
  email change flow.
- Changing a password requires `oldPassword` unless you're a superuser or a manager.
- Emails are returned only to superusers, to the record itself, to managers, or when
  `emailVisibility` is on. This is evaluated in SQL per row.
- `@request.auth.*` resolves to the signed-in record's values, except `password` and `tokenKey`,
  which are always null. Secret fields can't be filtered or sorted by anyone.

**Sign-in** compares against a real dummy bcrypt hash when no record matches, so unknown
identities take as long to reject as wrong passwords. Requests that start a flow by email
(verification, password reset) always return 204 and send the email in the background, so they
don't reveal which emails exist.

**Settings** (`meta`: app name, URL and sender; `smtp`) are stored in `_slonbase.params`. The SMTP
password is never returned by the API, and an empty password in an update keeps the stored one.
Mail goes through `lettre` with rustls (no OpenSSL), or to the log when SMTP is disabled.

Auth alerts, rate limits on sign-in attempts (on by default for new installations since
0.4.2) and the confirmation pages behind the email links (`/_/#/auth/...`, served by the
dashboard) came later.

### Phase 3: OAuth2, OTP and MFA

These follow PocketBase's endpoints and options, so the SDKs' `authWithOAuth2`, `requestOTP`,
`authWithOTP` and MFA flows work. State lives in three `_slonbase` tables (`external_auths`,
`otps`, `mfas`) that reference the collection with `ON DELETE CASCADE`; a record's rows are
deleted with the record, and expired OTPs and MFA sessions are pruned by the listener.

**OTP.** `request-otp` answers `{"otpId"}` for any email, and does the work for registered ones
(bcrypt hash, insert, email) in the background, so neither the answer nor its timing reveals
which emails exist. Only the hash is stored. An OTP is single-use (deleted with a `DELETE` whose
row count decides a race), expires after `otp.duration`, and is invalidated after 5 wrong
passwords. A record can have at most 9 active OTPs; further requests return the latest id without
a new email. Signing in with an OTP marks the record verified when it went to its email. The
email template gets `{OTP}` and `{OTP_ID}`.

**MFA.** When `mfa.enabled` and the record matches `mfa.rule` (empty: every record), a sign-in
with one method (`password`, `otp` or `oauth2`) answers `401 {"mfaId"}` instead of a token.
Signing in with a *different* method and that `mfaId` (body or query string) returns the token.
An MFA session is bound to its record, expires after `mfa.duration`, and is single-use. MFA
requires at least two enabled methods. Token refresh and impersonation don't ask for it.

**OAuth2.** Presets for Google, Apple, GitHub, GitLab, Discord, Microsoft, Facebook, Bitbucket,
Twitch, Spotify, Patreon, Kakao, X (`twitter`), Box, Gitea/Forgejo (`gitea`, gitea.com by
default), Gitee, Instagram (`instagram2`), Lark, Linear, LiveChat, mailcow (no default URLs),
monday.com, Notion, Planning Center, Strava, Trakt, WakaTime and Yandex, and `oidc`, `oidc2`,
`oidc3` for any OpenID Connect provider (configured URLs). Any preset's URLs can be overridden
(e.g. self-hosted GitLab or Forgejo). Linear and monday.com are read with a GraphQL query;
Trakt and Notion get the headers their APIs want; Linear's authorization doesn't do PKCE, so
it's off by default there and no verifier is sent.
- `auth-methods` lists each provider with a fresh `state` and PKCE pair (`S256`, on unless
  `pkce: false`) and an `authURL` ending in `redirect_uri=` for the client to complete.
- `auth-with-oauth2` exchanges the code (client secret in the form body, or with HTTP Basic
  authentication for Bitbucket, X and Notion, which want that; 15 s timeout, 1 MiB response limit) and fetches the provider's user, then picks the record: the linked one, else
  the signed-in record of the collection (linking a new provider to it), else the one with the
  provider's email, else a new one.
- **Only verified emails are used**, since an email links the provider account to an existing
  record: OIDC/Google need `email_verified`, Discord `verified`, Patreon `is_email_verified`,
  Kakao `is_email_valid` and `is_email_verified`; GitHub falls back to the verified primary
  email and Bitbucket asks for the confirmed primary one; Twitch returns only verified emails.
  Microsoft's `mail` and Spotify's `email` aren't verified, so they're ignored, and X gives
  none. Gitea and Gitee ask for the verified primary email; LiveChat needs `email_verified`,
  monday.com `is_verified` and WakaTime `is_email_confirmed`. Box, Linear, mailcow, Notion
  and Yandex only have addresses their users signed in with, so theirs are used; Strava, Trakt,
  Instagram, Lark and Planning Center give none. Bitbucket, Box, Linear, monday.com, mailcow
  and Planning Center accounts that aren't active can't sign in.
- An existing **unverified** record with the provider's email may belong to someone who signed up
  with an address they don't own: unless it's the signed-in record, its password is replaced
  with a random one and its token key rotated (signing out every session) before it's marked
  verified. PocketBase does the same but also verifies records whose email differs from the
  provider's; slonbase doesn't.
- New records are created through the records API as a guest, so the create rule, validation and
  realtime events apply. `createData` and the `mappedFields` (`id`, `name`, `username`,
  `avatarURL`) fill the record; an avatar mapped to a file field is downloaded (5 MiB limit) and
  checked against the field's types. The record is verified when the provider gave an email.
- The response adds `meta` (`id`, `name`, `username`, `email`, `avatarURL`, `isNew`, `rawUser`,
  `accessToken`, `refreshToken`, `expiry`).
- Client secrets are never returned by the collections API; an empty `clientSecret` in an update
  keeps the stored secret of the provider with the same name.
- `/api/oauth2-redirect` (GET, or POST for `form_post`) passes `{state, code, error}` to the
  realtime client whose id is the `state` and that subscribed to `@oauth2` (the SDKs' popup
  flow). When that client isn't connected to this instance, the message goes through
  `pg_notify` to the others.
- Linked accounts: `GET /api/collections/{c}/records/{id}/external-auths` and
  `DELETE .../external-auths/{provider}` (the record itself or superusers), PocketBase's
  pre-0.23 endpoints. PocketBase 0.23's `_externalAuths` collection isn't exposed.

The HTTP client is `reqwest` with rustls and ring, using the Mozilla roots (like the database and
SMTP connections), so the binary stays OpenSSL-free and works in a `scratch` image.

Superusers can use OTP and MFA like any auth collection (the dashboard's sign-in asks for the
code); `slonbase superuser otp <email>` issues an OTP without email (stored like a sent one,
single use), so MFA can't lock out an installation without SMTP.

**OpenID Connect without a user info URL** reads the user from the token response's
`id_token`, validated as PocketBase validates it:

- the audience (`aud`, a string or a list) must include the client id;
- `exp`, `nbf` and `iat`, when present, must hold within 5 minutes of leeway;
- with the `issuers` extra option (a list, or space-separated), `iss` must be one of them;
- with the `jwksURL` extra option, the signature is verified with the key of the token's `kid`
  from that key set, whose `alg` must be the token's: RSA (`RS256`/`384`/`512`,
  `PS256`/`384`/`512`) or Ed25519 (`EdDSA`), with `ring`.

Without `jwksURL` the signature isn't checked: the token came straight from the provider's token
endpoint over TLS, which OpenID Connect allows to stand in for it. With a user info URL, the user
comes from that endpoint and the `id_token` isn't used.

**Sign in with Apple** follows PocketBase's provider. Apple has no user info endpoint, so the
user always comes from the `id_token`, with stricter checks than OIDC's: `exp` is required,
`iss` must be `https://appleid.apple.com` and the signature is always verified with Apple's
keys (`https://appleid.apple.com/auth/keys`). The sign-in URL asks for
`response_mode=form_post`. Apple sends the user's name only once, with that first POSTed
redirect (`user`), so `/api/oauth2-redirect` keeps it for a minute by authorization code in
the unlogged `_slonbase.oauth2_names` table (migration 13), and the sign-in with that code
takes it. It's in PostgreSQL rather than memory because the redirect and the sign-in may reach
different instances. The client secret Apple wants is a signed JWT: see the generator below.

**Not yet:** VK, whose email comes with the token response rather than its user API. `oidc`
covers the providers that speak OpenID Connect.

### Phase 3: realtime and multi-instance sync

**Protocol.** `GET /api/realtime` opens a Server-Sent Events stream whose first event is
`PB_CONNECT` with a client id. `POST /api/realtime` sets that client's subscriptions:
- Topics are `collection/*` (or the legacy `collection`) and `collection/{id}`, optionally with
  `?options={"query": {...}, "headers": {...}}`.
- `query` can hold `filter` and `fields`.
- Events are named after the subscription string and carry `{"action", "record"}`, so
  PocketBase's SDK `subscribe()` works unchanged.
- The subscribe request's auth becomes the client's auth. A guest client may become
  authenticated, but switching to another user is rejected (403).

**Outbox.** Every record create, update and delete inserts a row into `_slonbase.events` in
the same transaction, stamped with the writing transaction's id (`xid xid8`, migration 15).
- Rolled-back changes are never announced.
- Every change of a collection table is announced, whatever makes it (see "Changes outside the
  records API" below); cascading deletes and the reference updates they cause too.
- Secret fields are stripped from stored events.
- Events are pruned after five minutes.

**Reading the outbox (1.7).** Before 1.7 each event's transaction also called
`pg_notify('slonbase', 'event:<id>')`. PostgreSQL takes one global lock on its notification
queue at commit for any transaction that notified, so every write committed one at a time:
pgbench measured 352 inserts per second with the trigger and its NOTIFY against 7,108 with the
trigger alone (32 clients, the same machine). 1.7 takes the notification out of the write:
- Each instance's poller remembers a snapshot (`pg_current_snapshot()`) and reads the events
  of transactions that weren't visible in it, in the same statement that takes the next
  snapshot: those in progress then (`xid = ANY(pg_snapshot_xip(prev))`) and those that started
  after (`xid >= pg_snapshot_xmax(prev)`), two index lookups. A transaction that commits late
  (with a smaller id than events already read) is still read once, and no event is read
  twice, whatever the commit order. (1.7 asked for `xid >= xmin AND NOT visible`, which is
  the same set, but scanned every event committed since the oldest open transaction began:
  a transaction left open for minutes made each read go through minutes of events. Event ids
  can't stand in for transaction ids, either: a soak test showed transactions taking their
  event's id before their own.)
- A write through the API wakes the local poller at once. Other instances hear of it through
  one coalesced `changed` notification, sent outside the write transaction, at most every 5 ms,
  and only while there are other live instances.
- The pool's connections set `slonbase.notify = off`, which the trigger reads; other programs
  (psql, a worker) don't, so their writes still notify. A commit through slonbase that may
  have run SQL writes (a hook's `$app.db()` in a transaction) wakes the poller and the
  announcer too. As a fallback for a lost notification, each poller also reads every 500 ms
  after activity, backing off to every 5 s while idle (1.8; before, every 500 ms always:
  about 170,000 queries a day per idle instance).

**Forwarded subscriptions.** A `POST /api/realtime` for a client whose stream is on another
instance is stored in `_slonbase.realtime_forwards` and announced (`forward:{id}:{instance}:{client}`).
The instance holding the client runs it with the original headers, authenticating its
`Authorization` itself, and writes the response back (`forwarded:{id}:1`); the others answer
`forwarded:{id}:0`. The forwarder relays the response, or answers 404 once every other live
instance said no (or after 5 seconds). Live instances come from `_slonbase.instances`, a
heartbeat every 20 seconds from each listener (and a `instance` notification when one starts,
so the others count it at once). Alone, an instance doesn't forward at all.

**Shared rate limits.** The guard counts requests in memory while `other_instances()` is 0,
and otherwise with one upsert per request on the unlogged `_slonbase.rate_limits` table (the
same fixed windows), falling back to memory if the database can't be reached.

**Shared email throttle.** The one-per-minute limit on requested auth emails (per account and
kind) is always counted in the database: one upsert on the unlogged `_slonbase.mail_sent`
table (migration 14) that claims the email only if the last one is a minute old, and returns
what was sent last otherwise (an OTP request answers with the code already sent). These
requests are rare, so there's no in-memory path while alone; memory is only the fallback when
the database can't be reached.

**Listener.** Every instance holds one dedicated connection that runs `LISTEN slonbase`. The
channel carries `changed` (new events: read them), `collections` and `settings` notifications
(sent when those change, so all instances reload them) and the forwarding messages above.

**Delivery.** The poller hands event ids to a delivery task, which reads them in batches (up to
500, one query) and checks the subscribers' rules and renders the messages for up to 8 events at
once, then sends them in event order, so each client still sees a record's create before its
update. (Delivering one event at a time, as before, fell behind under sustained writes; the soak
test found it.)

If the listener's connection drops, it reconnects with backoff, reloads collections and
settings, and wakes the poller, whose snapshot still covers what happened meanwhile (within the
five minutes events are kept). A test kills the listener's backend mid-stream to check this.

**Access checks.** Each subscriber is checked against:
- the `listRule` (collection topics) or `viewRule` (record topics)
- the topic's `filter`
- email visibility (auth collections)

All checks for an event run in one SQL query against a one-row table built from the event's
record values, so delete events can be checked after the row is gone. The query covers up to
50 subscribers.

Hidden fields go only to superusers, and secrets go to no one. A client whose buffer (512
events) fills up is disconnected.

`expand` in a subscription's options is applied to each event with the subscriber's auth and
rules for the related records (the same expansion as the records API).

**Changes outside the records API.** The outbox row (and, for connections other than slonbase's own, a `changed` notification) is
written by an
`AFTER INSERT OR UPDATE OR DELETE ... FOR EACH ROW` trigger on every collection table
(`_slonbase.record_change`, system migration 10), not by the API, so auth flows that write
columns directly, SQL from hooks, migrations and the SQL console, and other programs produce
events too. The trigger's arguments are the collection id, its secret columns (dropped from the
event) and its date columns (formatted with the API's `to_char`, so events carry exactly what
the API returns; a test compares them). The collection DDL recreates the trigger when those
columns change; migration 10 adds it to existing tables from the stored definitions. Restores
and PocketBase imports set `slonbase.realtime = off` for their transaction.

### Phase 3: relation paths and expand

**Relation paths** in filters and rules (`author.org.name`) compile to correlated subqueries
over the related tables, never JOINs, so they can't duplicate rows or skew counts.
- If every hop is single-valued, the path is a scalar subquery.
- If any hop or the final field is multi-valued, the path is a set compared with the usual
  all/any (`?`) semantics.
- A missing related record behaves like a single empty value, so `author.name = null` matches
  records without an author.
- Paths can end in a JSON path (`author.meta.key`) and have up to 6 hops.
- `@request.auth.relation.field` paths start from the caller's relation value, bound as a
  parameter (a JSON array for multiple relations), and then join like a record's path. A guest,
  or a caller whose collection doesn't have the field, gets null. When a rule is saved there is
  no caller, so the path is compiled against every auth collection that has the field.
- The related collections' API rules don't apply (as in PocketBase), but their hidden fields stay
  hidden from non-superusers, and secrets are never reachable.
- Rules that use paths are validated against the related collections when saved.

**Expand** runs after the main query:
- It loads each relation level with one query per path for all records, using `id IN (...)` and
  the related collection's `viewRule`.
- Back-relations (`{collection}_via_{field}`) use the source collection's `listRule`.
- Expanded records get the same field and email visibility as a direct request.
- It works on list, view, create and update responses, up to 6 levels and 5,000 related records
  per path.
- Unknown or forbidden paths are skipped silently, as in PocketBase.
- `fields=...,expand` keeps the expanded records, and `expand.author.name` picks in them.
- Once a path's relation field resolves, every record gets an `expand` object, empty if
  nothing was found, and a multiple relation with nothing found gets no key, as PocketBase
  answers (slonbase left `expand` out and gave `[]` before 1.6).

**The `fields` parameter** (`picker.rs`) is a port of PocketBase's `tools/picker`, checked
against PocketBase itself (see below):
- It's a middleware on every successful JSON answer, as PocketBase applies it in `e.JSON()`;
  lists pick their items in place and mark the answer, which saves parsing it again. Batch
  requests apply their own URL's `fields` to their bodies, and realtime to each record (one that
  can't be parsed sends the record whole, as PocketBase).
- PocketBase's tokenizer, `fields` parsing (a name keeps the spaces before its `:`), its
  argument reading (spf13/cast's `ToInt`: `0x10`, `0b11`, `010` octal, `1_0`, `12.7` is 12;
  `strconv.ParseBool` for the ellipsis) and its picking rules are reproduced, the quirks too:
  a field naming the key itself wins over deeper paths, a path into a non-object keeps the
  value, and `*` fills in the keys no field names.
- Fields are kept sorted, so the fields matching a key are a range: a crafted `fields` with
  thousands of entries over a 1,000-record page costs ~19 ms, where scanning them all for each
  key took ~160 ms.
- A `fields` that can't be parsed is a 400, as PocketBase answers, but a request that changes
  something gets it before it runs: PocketBase makes the change, then answers 400. Batch
  requests' own `fields` are checked when the batch is read, before its transaction.

**Excerpts** (`excerpt.rs`) port PocketBase's `excerpt` modifier: the HTML is parsed with
html5ever (an HTML5 parser, as Go's `x/net/html`) into a small arena DOM, and the text is
gathered with the same rules: excluded elements (`script`, `style`, `svg`, `img`, form
controls, `template`, ...), a space around every non-inline element, runs of `[\t\n\f\r ]`
collapsed, the early stop past `max+2` bytes (so multi-byte text gets shorter excerpts, as in
PocketBase), the cut at `max` characters and the `...`. Go's int overflow for a huge `max`
(nothing is gathered) is reproduced too.
- Nesting is bounded as in Go's `x/net/html`, which refuses a document once its stack of open
  elements passes 512: an element deeper than 512 (`html`, `body` and 510 `div`s are fine,
  511 aren't) fails the excerpt, and the answer is then sent unpicked, all of it, as
  PocketBase does. The parser is fed 16 KiB at a time and stops at once: HTML5 tree
  construction takes time quadratic in the nesting, and a stored 400,000-deep document would
  otherwise hold a CPU for minutes on every request for its excerpt.
- The text is gathered with an explicit stack rather than recursion, and excerpts are made on
  a blocking thread, off the async workers.

The excerpts were compared with PocketBase 0.40.5's on a corpus of editor output and corner
cases plus 5,300 random documents (tags of every kind, misnested and unclosed, entities,
comments, Unicode, odd whitespace) at 8 settings each: 42,400 excerpts, all the same but one
document's. There, `<template>` before any body content then a `<frameset>` inside the body,
Go's parser drops the text: it resets `frameset-ok` when it implies the `<body>`, which the
HTML spec doesn't (`x/net/html` v0.39 `afterHeadIM`); html5ever follows the spec and keeps
it. `compat/tests/excerpts.json` holds PocketBase's excerpts of the corpus and 300 of the
documents, and the compatibility suite checks slonbase against them in CI.

A sweep runs relation-path operands against every operator and value kind in PostgreSQL to check
the generated SQL is always accepted.

### Phase 3: files

**File fields** (`maxSelect`, `maxSize`, `mimeTypes`, `thumbs`, `protected`) store file names
(`{snake_case_name}_{random}.{ext}`). The files live at `{collectionId}/{recordId}/{name}` in
storage, and thumbnails at `{collectionId}/{recordId}/thumbs_{name}/{size}_{name}`. Keys are
validated part by part, so path traversal is impossible. Storage is the local filesystem (atomic
writes with a temp file and rename) or an S3 bucket (see below).

**Uploads** are multipart. JSON bodies are capped at 2 MiB and multipart requests at 128 MiB.
Each file is checked against `maxSize` (default 5 MiB) and `mimeTypes`. The MIME type comes from
the content: magic bytes, SVG sniffing, then text vs binary. The file name only refines the type
between text formats, so a text file named `x.png` is rejected by an `image/png` field.

**Files stay consistent with the database:**
- New files are written before the database transaction, and removed again if it fails.
- Files replaced or removed by an update are deleted only after commit, with their thumbnails.
- Deleting a record deletes the files of every deleted record, cascades included.
- Deleting a collection deletes its files.

**Serving** (`/api/files/...`):
- Images, video, audio and PDF are served inline. Everything else is an attachment (as in
  PocketBase).
- Every file gets `Content-Security-Policy: default-src 'none'; ...; sandbox` and `nosniff`, so an
  uploaded HTML or SVG file can't run scripts.
- Public files are cached for 30 days.
- Protected files need a file token (`POST /api/files/token`, a separate token type) and the
  record's `viewRule`. They're served with `private, no-store`.
- Thumbnails are allowed for `100x100` and the field's `thumbs` sizes only. They're generated
  once, off the async threads, with decoding limits (12,000 px, 256 MiB) against decompression
  bombs.

Single byte ranges (`Range: bytes=a-b`, `a-`, `-n`) are answered with `206` (seeking on disk,
a `Range` GET on S3), with the type sniffed from the file's start like a full response; several
ranges or `If-Range` get the whole file, an unsatisfiable range `416`.

**Removed file fields:** when a collection change removes a file field, its stored files (and
their thumbnails) are deleted after the change commits, so they don't stay in storage with
nothing pointing at them. The file names are read in the change's transaction, just before
the column is dropped (`store::apply_in`), for every path that changes collections: the API,
imports, hooks and migrations. Renaming a field keeps its files (fields are matched by id).
PocketBase leaves such files in storage.

### Phase 3: S3 storage

**Configuration** is PocketBase's `s3` settings section (`enabled`, `bucket`, `region`,
`endpoint`, `accessKey`, `secret`, `forcePathStyle`), stored with the other settings in the
database. Changing it switches the storage of every instance: they reload settings on the
`settings` notification. The secret is masked like the SMTP password. Enabling S3 doesn't move
existing files (neither does PocketBase). `POST /api/settings/test/s3` writes, reads and deletes a
test object with the saved settings and reports the S3 error.

**The client** is built in (`src/s3.rs`, a few hundred lines) instead of the AWS SDK, which would
add many dependencies and megabytes: AWS Signature Version 4 over the shared `reqwest`/rustls
client, so it works in the `scratch` image.
- Operations: PutObject (with the payload's SHA-256 signed; hashed off the async threads above
  1 MiB), GetObject streamed to the client, HeadObject for thumbnails, DeleteObject,
  ListObjectsV2 with pagination and DeleteObjects in batches of 1,000 for record and collection
  deletes.
- Virtual-hosted (`{bucket}.{host}`) or path-style (`{endpoint}/{bucket}`) URLs.
- One retry after a connection error, timeout or 5xx (every request is idempotent).
- Connect and read timeouts, and a 5-minute total limit except for downloads, which stream.
- Signing is tested against AWS's published examples; the client and the file flows are tested
  against MinIO (in CI) and s3s-fs.

A misconfigured S3 storage fails every file operation with its error rather than falling back to
the local directory, so files never end up somewhere unexpected.

**Private CAs:** `server.http_ca_file` (`--http-ca-file`) adds a PEM bundle's certificates to
the Mozilla roots of the shared outbound HTTP client, so it covers S3, OAuth2 providers, JWKS
and hooks' `$http`. It's read at startup and applies to the whole process.

**Multipart uploads:** `put_file` (backups, and any file from disk) uploads files over 64 MiB in
16 MiB parts (larger when a file would need more than 10,000), each read into memory and sent,
and retried, on its own; then `CompleteMultipartUpload` (whose 200 can carry an error). A
failure aborts the upload. Tested against a mock server and, in CI, MinIO.

**Presigned downloads:** with `s3.presignedDownloads`, `serve` does everything up to the
download hook as usual (record, rule and file token checks, thumbnail generation), then reads
the file's first 8 KiB from the bucket for its type (a 404 there is a 404 here; an empty file
falls back to a size check) and answers 302 with a SigV4 query-signed GET URL
(`UNSIGNED-PAYLOAD`, only `host` signed). `response-content-type`,
`response-content-disposition` and `response-cache-control` are signed in, so the bucket sends
what `serve` would. Public files are signed at the start of the hour for two hours, so the URL
(and the browser's cache of the download) is stable for an hour and the redirect is cached for
ten minutes; protected files are signed at the start of the minute for five minutes, and the
redirect is `private, no-store`. Signing is tested against AWS's query-parameter example, and the
flow against MinIO in CI (tampering with a signed parameter gets a 403 from the bucket).

### Phase 4a: admin dashboard

**The UI** was first PocketBase's dashboard, embedded unchanged to prove the API compatible; it
has since been replaced by slonbase's own (see "Dashboard redesign" below).

**System collections.** The UI manages superusers and linked accounts through the records API,
so like PocketBase they're collections: `_superusers` (auth) and `_externalAuths` (base). They're
created after the SQL migrations; rows of the private tables earlier versions used are moved in
the same transaction (superusers get new token keys, signing them out once). Superuser auth is
ordinary record auth, so OTP, MFA and password reset work for superusers too.

**API added for the UI:** the full settings model (merged recursively like PocketBase, with the
SMTP password and both S3 secrets masked and kept when left empty), `test/email` (sent
synchronously so SMTP errors are reported), scaffolds (without the auth email index, which
slonbase manages itself), the OAuth2 provider list, collection import (validated against the
registry as it will be, applied in one transaction), truncate, and crons (none yet).

**The SQL console** is off by default: on SQLite PocketBase's console can only touch its own
database file, but on PostgreSQL it runs with the role's privileges, which for a superuser role
include reading server files and running programs. When enabled (`--sql-console`), read queries
run in read-only transactions, and every query has a 3 minute statement timeout and returns at
most 1,000 rows.

**The Apple client secret generator** (`POST /api/settings/apple/generate-client-secret`,
superusers, as in PocketBase) signs the ES256 JWT Apple takes as a client secret, from the
team id, key id and `.p8` private key (`ring`), valid for up to 15,777,000 seconds (~6 months).
The dashboard's Apple provider form uses it. (Logs, backups, the Geo Point field, view
collections and slonbase's own dashboard came later.)

### Phase 4b: logs

PocketBase keeps logs in a second SQLite database so writing them doesn't contend with the data.
slonbase keeps them in `_slonbase.logs`, an **unlogged** table (no write-ahead log, so cheap
writes; emptied after a database crash and not replicated, which is acceptable for logs), and
never writes them on the request path:
- A middleware builds the entry after the response is ready (method, URL, status, execution time,
  user agent, referer, the caller's auth collection, optionally its id, and IPs) and queues it.
  The auth extractor fills in the caller through a request extension; error responses carry
  their message and internal details in a response extension.
- A background task writes the queue in batches (up to 500 entries or every 500 ms) with one
  `INSERT ... SELECT unnest(...)`. A full queue (10,000 entries, e.g. while the database is down)
  drops new entries rather than slowing requests.
- A `tracing` layer installed by the binary forwards warnings (level 4) and errors (level 8),
  with their fields, to the same queue.
- Levels and fields follow PocketBase (`data.status`, `data.auth`, `data.execTime`, ...), so the
  dashboard's views and filters work. Filters and sorts compile with the records' compiler,
  against the table described as a collection; `@rowid` sorts by an identity column.
- Settings: `maxDays` (0 disables logging; older entries are deleted every minute), `minLevel`,
  `logIP`, `logAuthId`. Dashboard files and successful health checks aren't logged.
- `remoteIP` is the connection's address; `userIP` comes from the first `trustedProxy` header
  with an address (rightmost of a list, or leftmost with `useLeftmostIP`).

- `maxDataSize` (0: 16 KiB) limits an entry's serialized `data`, as in PocketBase: a larger
  entry keeps the attributes that fit whole, in order, and gets `"__pb_truncated__": true`
  (PocketBase's marker, so its tools recognize it). Messages are cut at 8,000 bytes. It's
  applied when an entry is queued, so it covers requests, warnings and errors, and hooks'
  logger alike.

### Phase 4c: rate limits and superuser IPs

One middleware applies both, inside the logger (so rejections are logged). It resolves the
caller once and leaves it in a request extension for the auth extractor, so enabling it doesn't
verify tokens twice.

**Rate limits** match PocketBase:
- Labels, most specific first: `{collection}:{action}` and `*:{action}` for collection endpoints
  (the action names are PocketBase's, derived from the method and path rather than per-route
  bindings), then `METHOD /path` and `/path`. Rules match exactly, or as path prefixes when their
  label ends in `/`; the audience (`""`, `@guest`, `@auth`) must fit the caller.
- Fixed windows per rule and client IP; collection rules count per collection and endpoint.
- Superusers and `excludedIPs` aren't limited; changing the rules resets the counters, and idle
  counters are pruned by the listener.
- Counters are in memory while an instance is alone (like PocketBase, which runs as one). With
  several instances they're shared through PostgreSQL (see "Shared rate limits" under realtime):
  one round trip per limited request, paid only by setups that run more than one instance.

**Superuser IPs:** with a non-empty `superuserIPs`, requests with a superuser token and any
request to the `_superusers` collection (sign-in, OTP, password reset, records) get `403` from
other addresses, and a superuser's file token from them counts as a guest's. The client address
is the trusted proxy header's when configured, else the connection's.

### Phase 4d: backups

PocketBase zips its data directory. slonbase's data is in PostgreSQL, and `pg_dump` isn't an
option (not in the `scratch` image, and it must match the server's major version), so slonbase
exports what it owns itself:
- **What's in a backup:** a manifest (format version, last migration, slonbase version, the
  tables and their columns), the `collections` and `params` system tables (collections, settings,
  the auth secret), every collection's table, and the local storage files when files aren't in S3.
  Short-lived state (realtime events, OTPs, MFA sessions, logs) is left out.
- **Export:** `COPY (SELECT ...) TO STDOUT` in the text format (portable across PostgreSQL
  versions), all in one repeatable-read read-only transaction, so the backup is a consistent
  snapshot; records in `_seq` order so a restore keeps their order. Tables go to temporary files,
  which are zipped off the async threads (deflate, already-compressed files stored).
- **Storage:** the backups directory (`--backups-path`) or the `backups.s3` bucket. Archives are
  streamed to and from S3, never held in memory.
- **Restore:** the archive's manifest is checked (format, and not newer than this version's
  migrations), entries are extracted with zip-slip protection, then one transaction drops the
  collection tables, reloads the system tables with `COPY FROM STDIN`, recreates the tables from
  the restored collections and loads their data. Local files are swapped in by renaming the
  directory. Missing system collections are created, and every instance reloads collections,
  settings and the auth secret (a `restored` notification) and drops its realtime clients.
- **Locking:** a backup or restore holds the instance's lock and a PostgreSQL session advisory
  lock, so only one runs across all instances; the health check reports `canBackup` to superusers
  like PocketBase.
- **Schedule:** `backups.cron` is a 5-field cron expression in UTC, checked every 20 seconds.
  Each scheduled minute is claimed with a conditional update of a params row, so one instance runs
  it. Automatic backups beyond `cronMaxKeep` are deleted, oldest first.
- Downloads need a superuser file token (and an allowed IP); uploads are streamed to disk, have no
  size limit, and must contain a valid manifest.

**Encryption at rest** (`backups.encryption_key`, `src/backup_crypt.rs`): the finished zip is
sealed before it's stored, streaming, in 64 KiB chunks with AES-256-GCM (`ring`) under a key
derived with PBKDF2-HMAC-SHA256 (600,000 iterations, a random 16-byte salt). The header (magic
`SLONBENC`, format, iterations, salt, a random 7-byte nonce prefix) is authenticated with every
chunk, and a chunk's nonce is the prefix, its number and a last-chunk flag, so reordering,
dropping, truncating or changing anything fails. Encrypted backups keep their `.zip` names
(clients expect them) and are recognized by their magic: restores and uploads decrypt them into
the temporary directory first. The key is configuration, not a setting, so it's never inside
the backups it protects. (PocketBase backups aren't restored as backups, but
`slonbase import-pocketbase` takes a PocketBase backup zip.)

### Phase 4e: batch requests

`POST /api/batch` follows PocketBase: `{"requests": [{"method", "url", "headers", "body"}]}` as
JSON, or multipart with the JSON in `@jsonPayload` and files named `requests.{index}.{field}`;
responses are `[{"status", "body"}]`, and a failure answers `400` with
`data.requests.{index}` holding the failing request's error.
- The record handlers' work is split into `create_in`/`update_in`/`delete_in`, which run in a
  caller's transaction; single requests and batches use the same code. File changes are collected
  (`FileEffects`): new files are removed if the transaction fails, replaced and deleted files once
  it commits. Realtime events go to the outbox in the same transaction (the tables' trigger
  writes them), so they're delivered only if the whole batch commits.
- `PUT .../records` is an upsert (an update when the body's `id` exists).
- The batch's `Authorization` is used for every request (theirs are ignored), and rules apply per
  request. The whole batch is limited by `batch.timeout`; dropping the transaction rolls it back.
- Request hooks run per request. Their chains need `'static` closures, which can't borrow the
  transaction, so the batch's connection and transaction live in a task of their own and a
  hook's `next` sends its write there as a job (one at a time, in order; reads for upserts and for
  the current record that update/delete hooks see go the same way, so they see the batch's earlier
  writes). The batch commits with a final message; an error, a timeout or a hook's failure drops
  the channel (aborting the task on timeout), which rolls back. A hook's own `e.response` becomes
  that request's `{"status", "body"}`.
- Difference from PocketBase: `expand` is applied after the commit, so expanded relations show the
  batch's final state rather than the state at that request.

### Geo Point fields and view collections

- `geoPoint` is stored as `JSONB NOT NULL DEFAULT '{"lon":0,"lat":0}'`; input may use numbers or
  numeric strings, coordinates are range-checked, and a required point can't be `0, 0` (like
  PocketBase). The filter language gained function calls: `geoDistance()` compiles to the
  haversine formula (km), and `field.lon` / `field.lat` read the coordinates as numbers.
- A view collection is a PostgreSQL view over its query. On create or update the query is
  prepared (`SELECT * FROM (query) _q LIMIT 0`, in a transaction that's rolled back) to read the
  result columns, which become fields by type (numbers, booleans, dates, JSON for arrays and JSON,
  text otherwise); fields with the same names keep their ids and display options. The view casts
  every column to its field's storage, so records are read by the same code as tables. Views are
  read-only (create/update/delete rules are null and writes are refused), have no indexes, and
  sort by `id` where tables use their insertion order. Unqualified names resolve to collections
  (`search_path = slonbase, public` in the creating transaction), so PocketBase queries work.
- Stability: views are real PostgreSQL views, so a view can't be stale or half-broken at read
  time. Any change to a table drops the other view collections first and recreates them from
  their queries after it (re-inferring their fields), in the same transaction: table changes
  never fail because of PostgreSQL's view dependencies, views follow column type changes, and a
  change that would break a view (removing a column it reads) is refused as a whole, naming the
  view. Collection changes apply views last (and drop them first), as do backup restores, which
  recreate views from their definitions. Rows with a NULL id are left out, single-record reads
  take one row (so duplicate ids can't fail them), and at startup a view collection whose
  PostgreSQL view is missing is recreated.
- Security: a view query runs with the role's privileges on every read, the same exposure as the
  SQL console, so creating or changing one requires `--sql-console`.

### Dashboard, 1.7

- One sidebar replaces the top bar and the collections panel: the sections, the collections
  grouped (base, auth, system; a filter past six), a three-way theme switch and the account
  menu. It folds to icons (kept per browser, names in tooltips) and becomes a drawer below
  920px, under a small bar with the menu, the wordmark and search.
- Warm, muted tokens: paper and stone neutrals, a sage accent (`#4f7a6c`; the earlier defaults
  and PocketBase's still mean "the theme's own"), earthy status colors, and a warm charcoal
  dark theme. Surfaces are told apart by soft shadows and hairlines rather than borders.
- Tabs are a segmented row of pills with a raised active one; the records grid has no outer
  border, a translucent sticky header and taller rows; the record editor is a drawer on the
  right (`Modal size="drawer"`), so the list stays in view.
- The overview leads with a hero: the greeting, the day's state and actions, beside the
  requests of the last 24 hours as a smooth area chart (a Catmull-Rom curve through the hours,
  hover for each hour). Below: four metric tiles, the collections ranked by their records with
  a bar for their share, and a health checklist.
- The sign-in page is split: a sage panel with what slonbase is, beside the form.

### Dashboard redesign (0.x)

- Its own design: a top navigation bar with an underline indicator, a collections panel,
  collection pages with tabs (records, schema, rules, sign-in, indexes, API docs, settings) that
  edit one draft saved from a floating save bar, records edited in a centered sheet with a
  metadata column, a ⌘K command palette, and an overview page.
- Visual language: neutral surfaces (warm off-white or near-black), borders rather than shadows,
  small radii, solid ink primary buttons, outlined chips, a monochrome logo and collection marks,
  and a single muted accent (slate by default, `meta.accentColor` to change it) used only for
  focus, links and selection. Status colors are muted and appear as text or dots. No gradients.
- Motion is quiet: short fades and 4px rises for pages and lists, sliding underlines for the
  navigation and tabs, a sliding thumb for segmented controls, switch transitions, and sheet and
  toast fades. It respects `prefers-reduced-motion`. Entrance animations use
  `animation-fill-mode: backwards`: a transform that stays applied would make the element the
  containing block of `position: fixed` descendants, so overlays are also rendered into `<body>`
  through a portal.
- No build step: plain ES modules with Preact and htm vendored (~17 KB), embedded by `build.rs`
  and gzipped. Every file is revalidated with an ETag (an FNV hash computed at build time) since
  names aren't hashed. `SLONBASE_DEV_UI_DIR=ui/app` serves the files from disk while working on
  them. The Content-Security-Policy allows only the dashboard's own scripts.
- The pages behind email links (`#/auth/confirm-verification/{token}`, password reset, email
  change) and superuser password reset live in the same app, since emails link to `/_/#/auth/...`.

### Phase 5a: Rust extensions

- `slonbase::cli::run(Extensions)` is the whole command line; the binary passes no extensions.
  `Extensions` holds `Hooks` and an `axum::Router<Arc<App>>` merged into the server's routes.
- A hook is `Fn(E, Next<E>) -> Future<Result<E, ApiError>>`; `next.run(e)` runs the remaining
  handlers and then the action (a `'static` closure owning everything it needs), so hooks can work
  before and after the action, or skip it. Handlers can be limited to collections (id or name).
- Request hooks wrap the record handlers. For updates and deletes, the record the hook sees is
  loaded with the caller's rule first (404 otherwise, so hooks never see records the caller can't
  touch); for updates it's merged with the request body, and only keys that differ from the stored
  record are written. Batch requests don't run request hooks (their writes share a transaction).
- After-success hooks get every change of a transaction (`FileEffects.changes`, including
  cascades and relations cleared by a delete) once it commits, from `settle`. They're local to the
  instance that made the change, unlike realtime events, so each change runs them once.
- Cron jobs are checked by the listener's 20-second tick; each matching minute is claimed per job
  in `_slonbase.params`, so one instance runs it.
- The record service (`Core::save_record` and friends) runs with `RequestAuth::system`, a
  superuser without a record, through the same `create_in`/`update_in`/`delete_in`. Filter
  placeholders (`{:name}`) are turned into literal tokens by the tokenizer, so values can't change
  the expression.

### Phase 5b: JavaScript hooks

- QuickJS through `rquickjs` (pure C, compiled with `cc`; prebuilt bindings cover the musl
  targets), behind the default `js` feature. The API is PocketBase's JSVM API, implemented in
  `src/js/prelude.js` on top of three natives: `__native_call(name, argsJson)`,
  `__native_next(stateJson)` and `__native_log`. Values cross as JSON, which keeps the binding
  small and the API in one JavaScript file.
- `hooks.pool_size` threads each own a runtime that evaluated the prelude and every `*.pb.js`
  file, so handler `i` is the same function in every runtime. The first runtime reports the
  registrations, which become ordinary Rust hooks, axum routes (`{path...}` becomes `{*path}`)
  and cron jobs.
- A handler runs on one thread. `e.next()` sends the event state to the async side, which runs the
  rest of the chain and the action and answers on a channel; the thread blocks meanwhile. Natives
  block on the tokio runtime handle (the threads aren't runtime threads, so `block_on` is allowed).
  If the request is dropped, the reply channel closes and `e.next()` throws.
- Database access: `$app` maps to the record service; `$app.db().newQuery()` binds `{:name}`
  placeholders as SQL literals (text dollar-quoted with a tag it doesn't contain) and reads rows
  with `WITH _q AS (...) SELECT json_agg(_q)`, which also works for `INSERT ... RETURNING`.
  Each query runs with `search_path = slonbase, public` (in a transaction of its own, or with
  the caller's path restored after). `$dbx` and the query builder are JavaScript in the
  prelude, a port of dbx's PostgreSQL builder: they produce SQL text with `{:pN}` parameters,
  so all binding stays in one place. `$app.recordQuery()` wraps the built query as
  `SELECT <the record columns> FROM (<query>) t`, to read rows as records.
- Errors: `ApiError` and its subclasses keep their status, message and data; other exceptions answer
  400 "Something went wrong..." and are logged with their stack.
- `routerUse` handlers are `Hooks::on_request` handlers, a layer inside the guard (so rate limits
  and IP rules apply first) around the whole router. JSON and form bodies (up to 32 MiB) are read
  for `e.requestInfo()` and handed on as a fresh body; other bodies stream through unread, so
  uploads aren't buffered twice. A handler's own response wins; otherwise the router's, with the
  handler's `setHeader` headers added. Without handlers the layer is a pass-through.
- The pool grows: handlers nest (middleware → route → `$app.save` → model hook), and with every
  runtime waiting on another a fixed pool would deadlock. Each job counts as queued until a runtime
  takes it; when fewer runtimes are idle than jobs are queued, an extra runtime starts (up to 256
  in all) and stops after 30 s without work.
- Model hooks (`on_record_create`/`_update`/`_delete`) run in `create_in`/`update_in`/`delete_in`,
  so every write path has them. Handlers get the record (the body over the defaults, or over the
  current values; never the token key or a password hash) and their changes are merged back into
  the body, which then goes through the usual validation, so a hook can't store what the API
  wouldn't. Update and delete read the record for the hook without `FOR UPDATE` and take the row
  lock afterwards: a handler that writes the record through another connection doesn't wait on
  its own transaction (that read is a moment older than the locked one).
- `on_record_validate` runs in the write functions once the record's own validation passed, on
  the final values (no secrets); its changes are ignored, an error rejects the write.
- Execute hooks (`on_record_create_execute`, ...) wrap the statement that writes the record
  (with the create rule check and the realtime event). Hook actions are `'static`, so they can't
  hold the transaction: the action sends the record over a oneshot channel to the write, which
  runs in the caller's future, and waits for its result; `tokio::join!` polls the handlers and
  the write together. A failed statement always fails the write (the transaction is aborted
  anyway), and a write whose `next` never ran fails with "cancelled by a hook". Changes the
  handlers made go through `apply_body` and `validate` again.
- `onModel*` exist only in JavaScript: the prelude registers each handler on the record hook
  (tags other than `_collections`) and the collection hook (no tags, or `_collections`) with a
  builder that picks the event's shape and adds `e.model`. These registrations are sent after
  all others, so they run inside the record and collection handlers' `next`, as PocketBase's
  record hooks (priority -99) wrap its model hooks.
- After-error hooks: `create_in`/`update_in`/`delete_in` record a failed write (not 403/404,
  which PocketBase rejects before its save) in `FileEffects`, and `settle` reports those, plus
  the writes of a transaction that failed, in a background task once it rolled back. Updates and
  deletes report the stored record (read then), with an update's data applied.
- Collection hooks run in the collections API under the schema lock: the handlers get the full
  serialization (the one updates merge into), and what they return is parsed and goes through
  `validate::prepare` like the request, with the id kept. Imports run them for each collection they create, update or delete. The after-success
  hooks get the saved collection (secrets masked) in a background task. The API's changes also
  run `on_collection_validate`, the execute hooks around `store::apply` (no channel needed:
  the change runs on its own connection, so the action owns it), and the after-error hooks.
- Mail: every email goes through `Core::send_mail_for`/`send_mail_now`, which run `on_mailer_send`
  around the mailer; auth emails pass their kind and record (without the password hash and token
  key). Handlers scoped to collections only see auth emails of those; the per-kind JavaScript hooks
  are `on_mailer_send` handlers that skip other kinds without entering JavaScript.
- Watching: the server polls a fingerprint of the hooks directory (paths, sizes and mtimes; hidden
  entries and `node_modules` skipped) every second; after a change settles it shuts down
  gracefully (at most 5 s, as realtime streams never end on their own) and `exec`s itself with the
  same arguments, keeping its PID for systemd and Docker.
- Files: a route's multipart uploads and `$filesystem` files (`fileFromBytes`, `fileFromPath`,
  `fileFromURL`) live in the running
  job's thread-local map, by token; JavaScript sees `{_upload, name, originalName, size}`. When
  `$app.save()` meets such values in a field, the native side turns them into `field+` uploads
  (after the kept names) for the record service, so they follow the usual file rules
  (`maxSize`, `mimeTypes`, cleanup if the save fails). The map is cleared when the job ends.

### `@collection` lookups and new device alerts

- `@collection.name[:alias].path` in rules and filters: each distinct `name:alias` becomes a
  `LEFT JOIN name __cN ON TRUE` inside one `EXISTS (SELECT 1 FROM (SELECT 1) __one ... WHERE expr)`
  around the whole expression, so it's true when some combination of rows matches, and the
  `(SELECT 1)` row keeps parts of the expression that don't use the lookup working when the
  other collection is empty. Paths can go through relations of the looked-up collection. At
  most 6 lookups per expression, to bound the joins.
- Sign-ins (password, OTP, OAuth2, after MFA) record a device in `_slonbase.auth_origins`: a
  SHA-256 of the client IP (trusted proxy aware) and user agent, plus a hash of the record's
  token key. Rows with another key hash are dropped first, so anything that rotates the key (a
  password change, a reset) forgets the devices without hooks in each of those paths. A new
  device sends `authAlert.emailTemplate` (`{ALERT_INFO}`: time, IP, user agent) unless it's the
  record's first known device. The last 5 are kept; a failure here is logged and never blocks
  the sign-in. Unlike PocketBase, origins aren't a `_authOrigins` collection yet.

### `strftime()` in filters

PocketBase's filters expose SQLite's `strftime(format, [time-value, modifiers...])`, so filters
and rules written for it use SQLite's time values, modifiers and format codes. slonbase
reproduces them exactly rather than mapping to PostgreSQL's date functions, whose results differ
(SQLite's month arithmetic overflows into the next month, numbers are Julian days, invalid input
gives NULL, and so on):

- `_slonbase.strftime(format, value, modifiers, now_ms)` (migration 12) is a PL/pgSQL port of
  SQLite's `date.c`: a time is a Julian day in milliseconds and/or its fields, with SQLite's
  parsing, rounding, error handling and printf. `localtime` and `utc` take the server's zone to
  be UTC. `now` is the request's time. It is checked against SQLite 3.53 (the version PocketBase
  0.40 bundles) on tens of thousands of random cases; `tests/strftime_cases.tsv` keeps a
  sample of SQLite's results for CI.
- That function costs ~50 µs a row, so date fields with the common format codes and modifiers
  (`+N days|hours|minutes|seconds|months|years`, `start of ...`, `weekday N`) compile to
  `to_char()` and interval arithmetic instead (~0.6 µs a row), guarded to years 2-9999 where
  both calendars agree, with the function as the fallback. `tests/strftime_date_cases.tsv`
  checks that both give SQLite's results.
- NULL results are `''`, as in PocketBase. Multi-valued relation paths give multi-valued
  results (`strftime('%Y', tags.created) ?= '2024'`).

### PocketBase import

- `slonbase import-pocketbase` opens PocketBase's `data.db` read-only with `rusqlite` (SQLite
  bundled, so nothing to install) on a blocking thread; a backup zip is extracted to a
  temporary directory first. Nothing in the PocketBase data is written.
- The schema goes through the collections import (`import_definitions`, shared with
  `PUT /api/collections/import`): `_collections` rows become PocketBase's API JSON (the row's
  columns over its `options`), base and auth collections in one transaction, then each view on
  its own, so a view with SQLite-only SQL is reported and skipped instead of failing everything.
  The CLI process runs with the SQL console on: the operator already holds the database.
- Records are read in `rowid` order (chunks of 500) and inserted with multi-row `INSERT`s, one
  transaction per collection, values converted by field type (0/1 booleans, JSON text for
  multi-value fields, ...) and written with the usual casts. Hooks, validation and realtime
  events are skipped on purpose: this is a copy of data that was valid in PocketBase. Superusers
  and `_externalAuths` rows use `ON CONFLICT DO NOTHING`.
- Files are copied (read and put, never moved) under the same keys; thumbnails and
  `.attrs` files are left out. Settings (a JSON blob in `_params`) go through the settings
  update, section by section of a known list.
- Two general fixes came out of it: indexes may be partial (PocketBase's auth collections have
  `... WHERE email != ''`; the condition is parsed and re-emitted, columns compared with
  literals only), and JSON numbers round-trip exactly (`serde_json`'s `float_roundtrip`;
  without it `23.400000000000002` read back as `23.4`).

### Performance

Where the time goes was measured, not guessed: CPU split between slonbase and PostgreSQL
under load (`/proc` ticks per process), instruction profiles of the server (callgrind on the
`profiling` build profile), and `EXPLAIN ANALYZE` and pgbench on the database side.

- **Allocator.** The release binaries are static musl builds, and musl's allocator serializes
  threads on every allocation: the 1.2.0 binary answered lists at a quarter of the speed of
  the same code built for glibc, with slonbase at 250% CPU and PostgreSQL idle. The binary
  (`src/main.rs`, not the library) uses mimalloc.
- **Prepared statements** (`db::TextQuery`). PostgreSQL parsing and planning every query was
  most of its work for small queries: pgbench runs a lookup by id 2.7× faster prepared.
  Queries now run as statements prepared once per connection, but only SQL seen before
  (`seen_before`: a process-wide set of SQL hashes), so one-off SQL (a filter typed once, or
  an attacker's stream of distinct filters) runs unprepared as before and can't fill the
  caches; each connection keeps at most 256 statements, and every connection's are dropped
  when collections change. A statement whose result type changed under it (a table altered by
  another program) is dropped and the query run unprepared. List pages pass `LIMIT` and
  `OFFSET` as parameters, so every page of a list shares a statement. Lookups whose SQL only
  depends on the collection (the auth record of a token) use deadpool's cache directly.
- **List totals** (`count_cache`). `count(*)` visits every matching row (2.4 ms for 10,000,
  against 0.3 ms for the page itself), so totals are kept between requests: keyed by the exact
  count SQL and parameters (the rule's, so the caller's id), valid while the data generation
  they were counted at is current, and at most 5 s. The generation moves on every change any
  instance or program makes (new outbox events read by the poller, and other instances'
  `changed` notifications), at once on this instance's own commits (`Hooks::committed`), on collection changes, restores and imports,
  and with a `changed` notification after SQL the triggers don't see (the SQL console, and
  `TRUNCATE` from hooks). It's read before counting, so a write committed during the count
  leaves the entry stale, not wrong. Not cached: view collections (their query may read any
  table), counts touching a view, and SQL that depends on the request's time (`@now` and the
  other date macros, `strftime()`: `Params::time_dependent`). Bounded to 4,096 entries and 8 KiB
  keys.
- **Lists written from the rows.** When nothing changes records on the way out (no list or
  enrich hooks, no `expand`, no `fields`), `access::RecordRow` serializes each row as
  `output_record` would build it, without a map per record (the maps, their keys' hashing and
  the copy into the response were a third of a list's instructions). A test compares both
  paths byte for byte over every field type. Hidden emails are removed keeping the other
  fields' order (`shift_remove`; it was a swap before).
- **The dashboard** is served under `/_/v/{build hash}/`, cached for a year as immutable,
  since a new build has another hash; `index.html` (revalidated) links its files there and
  preloads every module (`modulepreload`), so the first load fetches them in parallel instead
  of as each import is found, and later loads only revalidate the page. Pages most visits
  never open (the diagram, Logs, Health, the SQL console, Projects, the API docs) load when
  first shown (`lazy()` in `core.js`), and the build leaves them out of the preloads.
- **In the dashboard,** a list seen before is shown at once from memory while the fresh one
  loads (`remembered` in `api.js`: 40 responses, this tab only, cleared when the signed-in
  account changes); record pages over 80 rows draw the rows in view and 15 on each side, with
  spacer rows for the rest (`useRowWindow`); deletes and saves show in the table before the
  server answers, and a refused delete puts the rows back.

- **Writes, 1.7.** A create through the API ran as fast as pgbench's insert with the
  realtime trigger, and that was the limit: the trigger's `pg_notify` made commits take
  PostgreSQL's notification-queue lock one at a time (pgbench, 32 clients: 352 inserts/s
  with the NOTIFY, 7,108 without it). The outbox is now read from snapshots (see "Reading the
  outbox" under realtime), so writes commit in parallel again; one coalesced notification
  tells the other instances, if there are any.
- **Creates in one round trip, 1.8.** A create through the API sent BEGIN, its INSERT and
  COMMIT (pgbench on the same machine: 16,911 transactions/s that way, 28,406 single
  INSERTs). When nothing can undo the INSERT once it ran (no execute hook around it, no create
  rule checked against the inserted record), it runs without a transaction, which is the
  same thing in one round trip (`creates_without_transaction`). Updates keep theirs: they lock
  the current row to apply modifiers (`views+`). Deletes can cascade.
- **Compression, 1.7** (`server::compress_json`). JSON answers are built in memory, so the
  whole body is gzipped in one pass (level 1, zlib-rs, a compressor kept per thread and reset)
  and sent with its `Content-Length`: a 30-record page goes from 14.5 KB to 2.2 KB for about
  25 µs. The first version used tower-http's streaming `CompressionLayer` (miniz, level 4,
  brotli or gzip) and cost lists 22% of their throughput on the benchmark; this one costs
  about 8%. Measured on that page: miniz at level 4, 218 µs; zlib-rs at level 2, 48 µs
  (1.7 KB); Rust's brotli 64 µs at quality 1 (1.8 KB) and 180 µs at quality 4 (1.5 KB), so
  brotli isn't offered. Never compressed, against BREACH (a page that reflects attacker text
  next to a secret leaks it through the compressed length): answers on the sign-in,
  refresh, impersonation, file token, setup and Apple client secret paths, and any JSON
  answer containing `"token"` (one scan of the bytes), which covers custom routes that sign
  users in. Files and `no-transform` answers are left alone.

### Tried and left out

- **PostgreSQL building list responses.** A prototype (in 1.2's cycle) had PostgreSQL write each
  record's JSON for lists without hooks, `expand` or `fields`, and sent the rows as they came.
  On the benchmark (`bench/`, alternating builds), plain lists of 30 got about 15–20% faster;
  sorted, filtered and signed-in lists about 5%, within the run-to-run noise. Not worth a
  second serializer that every field type would have to match byte for byte (PostgreSQL also
  writes some numbers differently, `1e+21` against `1e21`). Writing the JSON in Rust straight
  from the rows (above) gets most of the gain with the same bytes.

## Configuration

Precedence: flags > environment variables > config file > defaults. Unknown keys in the config
file are rejected, so typos fail loudly. Values from flags or env vars are never written back to
the file.

**Encryption at rest** (`server.encryption_key`, `src/secrets.rs`). The `settings` and
`auth_secret` rows of `_slonbase.params` are stored as `{"sealed": base64}`: AES-256-GCM with
the key's 32 bytes, a random 96-bit nonce before the ciphertext and tag, which is PocketBase's
`security.Encrypt` layout, so a PocketBase import with the same key opens its settings (CI
imports from a real PocketBase with `PB_ENCRYPTION`). Every read and write of those rows takes
the core's key. On start with a key, plain rows are sealed (with a compare-and-set, so instances
starting together don't fight); a sealed row without the key, or one the key can't open, is a
`KeyError`, which stops startup rather than being retried. A restore checks, inside its
transaction, that the restored rows open with the installation's key, and rolls back otherwise.
OAuth2 client secrets in collection definitions are stored as `{"sealed": ...}` in place of the
string (`secrets::seal_collection`, applied by `store`'s `_with` functions, which every caller
passes the key to; the forms without it remain for library users), so a definition can't be
read as a collection unopened; projects' role passwords are stored as `sealed:<base64>`
(`seal_text`). Plain ones are sealed on start (`seal_plain_rows`), and a restore checks the
collections' secrets too. `slonbase rekey` rewrites all of it, in the main database and every
project's, in one transaction each.

## Distribution

- **Static musl binaries** for `amd64` and `arm64`, cross-compiled in CI with `cross`.
- **`.deb` and `.rpm`** built with nfpm (`packaging/nfpm.yaml`). They contain:
  - a hardened systemd unit
  - a dedicated `slonbase` system user
  - `/etc/slonbase` (config) and `/var/lib/slonbase` (data)
  - `/etc/default/slonbase` for environment settings

  The `.deb` recommends `postgresql` but doesn't require it, so remote databases work without
  installing a local server. Purging removes the config and local data, never the database.
- **Docker**: `Dockerfile` builds from source (used by Compose). Releases use
  `packaging/Dockerfile.release`, which copies the CI-built binaries into a `scratch` image, so
  multi-arch images need no emulation.
- **Releases**: bumping the `Cargo.toml` version on main makes one. When CI passes on main and
  that version has no tag, `release.yml` tags the commit and publishes a GitHub release
  (packages, tarballs, `SHA256SUMS`), `ghcr.io/ayeshrodz/slonbase` and the apt and dnf
  repositories. Pushing a `v*` tag by hand works too; it must match the `Cargo.toml` version.
- **apt and dnf repositories**, signed with the key in the `REPO_GPG_PRIVATE_KEY` secret and
  hosted on GitHub Pages (`scripts/build-repo.sh`: `apt-ftparchive`, `createrepo_c`,
  `rpmsign`). Each release rebuilds them, so users get updates with `apt upgrade` or
  `dnf upgrade`.

## Testing

- Unit tests for config, ids, validation and migrations.
- Integration tests against real PostgreSQL (`SLONBASE_TEST_DATABASE_URL`). Each test creates and
  drops its own database. They cover the full wizard flow, reconnecting to an existing install,
  env-configured databases, concurrent migrations, and refusing foreign or newer schemas.
- CI runs them against PostgreSQL 14 (the oldest supported) and 17.
- S3 tests run against an S3-compatible server when `SLONBASE_TEST_S3_ENDPOINT` is set (MinIO in
  CI): the client (pagination, batch deletes, bad credentials), and files through the API with S3
  enabled in the settings (serving from a second instance, thumbnails, replacement and delete
  cleanup, the test endpoint, failed uploads that save nothing, switching back to local).
- File tests cover multipart uploads with form and JSON values, serving headers, thumbnails,
  traversal attempts, size/type/count validation with nothing left behind, append/remove/replace
  with cleanup, and protected files with file tokens.
- Realtime tests read real SSE streams. They cover rules and filters, hidden fields, rolled-back
  writes, single-record topics, auth subscriptions, delivery across two instances (events,
  collections and settings), writes made outside the API (psql) and outside slonbase's
  pool, and catching up after the listener connection is killed.
- Batch tests cover creates, updates, upserts and deletes in one batch (with expand), all-or-nothing
  failures with the failing request's error, rules with the batch's auth, multipart files (and their
  cleanup on failure), and limits.
- Backup tests cover creating (names, duplicates), restoring in place (order, JSON, special
  characters, files, settings, dropped collections) and into another installation (its auth
  secret replaced), downloads with file tokens, uploads (only valid backups), deletes, automatic
  backups with `cronMaxKeep`, and backups in S3.
- Guard tests cover rate limit labels and audiences, per-client windows, excluded IPs,
  superusers, resets, and the superuser IP allowlist (requests, sign-in, file tokens).
- Log tests cover request entries (errors, auth, user agent, referer, proxy IPs, auth ids),
  what isn't logged, filters, sort, stats, settings (minimum level, IPs, off), cleanup and
  truncation.
- Dashboard tests cover serving the UI (compression, caching, CSP), scaffolds, providers, the
  settings model (recursive merge, kept secrets, validation), the test email, the SQL console
  (off by default; read-only reads, committed writes), import (cross-references, deleteMissing,
  atomic failure) and truncate.
- Auth method tests run a mock OAuth2 provider (code exchange, user info, avatar) and cover
  sign-up with mapped fields and avatars, linking by provider id, signed-in record and verified
  email, the unverified-record takeover protection, the create rule, linked accounts, masked
  client secrets, OTP limits and single use, MFA rules and sessions, and the `@oauth2` redirect
  over realtime.
- Auth tests cover auth collection definitions, sign-up/sign-in, rules with `@request.auth`,
  email visibility, password and email changes with managers, the verification, password reset
  and email change flows (with a capturing mailer), impersonation and settings.
- API tests cover superuser auth, collection CRUD with real schema changes, records with
  filtering/sorting/pagination/modifiers, API rules, relations (validation, cascades, required
  references), and the filter type matrix.
- Compatibility (`compat/`): Node's test runner with the official `pocketbase` SDK (pinned)
  against a server started on a fresh store by `compat/run.sh`: records (CRUD, filters,
  pagination, expand, modifiers, validation codes), auth (sign-up, sign-in, refresh, rules, email
  visibility, password changes, impersonation), collections, settings, files (uploads,
  thumbnails, protected files), batch requests and realtime. CI runs it against slonbase and
  against the PocketBase release (checksum-verified), so an expectation only stays if PocketBase
  meets it too. Known, deliberate differences are left out of the shared assertions: emails are
  unique regardless of case in slonbase (PocketBase compares them as written).
- Differential runs (`compat/diff/run.sh`, 1.7): PocketBase and slonbase started side by side
  on fresh stores, seeded the same way, then sent the same requests (records, lists, filters,
  expand, `fields` and excerpts, auth, errors, view dry runs); the answers are compared as JSON
  after hiding what can't match (ids, dates, tokens), and any other difference fails CI. Each
  run also sends 1,000 randomly generated HTML documents through both servers' `:excerpt`.
  Where slonbase answers differently on purpose (a dry-run view's field ids, inferred from
  PostgreSQL's types instead of cloned from the source fields), a case compares the answers'
  shape instead.
- PocketBase import (`compat/import/run.sh`): seeds a real PocketBase through the SDK (most field
  types, relations, an auth collection, files, an OAuth2 link, two views), snapshots it through
  its API, imports its backup zip into a fresh slonbase and checks the snapshot through
  slonbase's API, including sign-ins with the old passwords.
- Upgrades (`compat/upgrade/run.sh <version|previous>`): downloads a release (checksum-verified),
  sets it up and seeds it through the SDK with what every release has (every field type, an
  auth collection with files, a view, relations, settings), snapshots its API, then starts this
  build on the same database, storage and config. The checks: everything in the snapshot comes
  back unchanged (newer keys are allowed), files byte for byte, old tokens and passwords work,
  writes, rules, the unique index, expands, the view, cascades, realtime and backups work, and
  nothing is logged as an error once it's up. CI runs it from 0.1.0 (every system migration)
  and from the newest older release.
- Benchmarks (`bench/`): the same seeded data and HTTP workloads on slonbase, PocketBase and PostgREST,
  with the server and the load generator on separate CPUs; results and how to read them are in
  [bench/README.md](../bench/README.md).
- Soak tests (`soak/`): sustained mixed traffic (reads, writes, uploads, token refreshes and
  realtime) on two instances sharing a database, failing on request errors, missed or late
  realtime events, memory growth, latency drift or connections left in use. CI runs them weekly
  for 20 minutes; see [soak/README.md](../soak/README.md).

## Roadmap

0. **Foundation** (done): server, config, setup mode and wizard, system migrations, packaging,
   CI and releases.
1. **Collections** (done): schema management, CRUD, filter/sort/pagination, API rules,
   superuser auth.
2. **Auth** (done): superusers and auth collections, password sign-in, verification, password
   reset, email change, impersonation, settings and mail; OAuth2, OTP and MFA (done).
3. **Realtime** (done: `LISTEN/NOTIFY` outbox, multi-instance sync), **relation paths and
   expand** (done), **files** (done: local or S3 storage, thumbnails, protected files).
4. **Admin dashboard** (done: system collections, settings; now slonbase's own UI), **logs**,
   **rate limits**, **backups** and the **batch API** (done).
5. **Extensibility** (done): Rust library API (hooks, routes, cron jobs, record service) and
   JavaScript hooks with PocketBase's API.
6. **Operations** (done in 0.6): realtime triggers on every table (changes from any source),
   subscriptions forwarded between instances, shared rate limits, `@request.auth` relation
   paths, Prometheus metrics, upgrade tests from every release.
7. **Parity details** (done in 0.7): `strftime()` in filters, deleting files when a file field is
   removed, OIDC `id_token` validation, Sign in with Apple and its client secret generator,
   the request log's `maxDataSize`.
8. **Hardening** (done in 0.8): S3 multipart uploads and a custom CA, backup encryption at rest,
   soak tests (and the realtime delivery fix they led to), a Health page and a schema diagram in
   the dashboard.
9. **1.0** (done): a documentation site, a [stability promise](stability.md) for the API,
   configuration and on-disk format, and setups for hosting platforms (Render's is one click; the
   Coolify, Dokploy and Railway ones are ready for their catalogs, see `deploy/README.md`).
10. **1.1** (done): a security review of the whole platform, with its fixes. **1.2** (done):
    presigned S3 downloads, Bitbucket, Twitch, Spotify, Patreon, Kakao and X sign-in, and the
    auth email throttle shared by the instances. **1.3** (done): the settings and the signing
    secret encrypted at rest, and lists 5 to 8 times faster (see [performance](#performance)).
11. **1.4–1.6** (done): fifteen more OAuth2 providers (every PocketBase preset but VK), any
    regular expression as an autogenerate pattern, and PocketBase's `fields` picker with
    nested paths and HTML excerpts, compared against PocketBase's own output.
12. **1.7** (done): a redesigned dashboard; writes that commit in parallel (the outbox read by
    snapshot instead of a NOTIFY per write); gzip for JSON answers; previews of view
    collections; a differential runner against PocketBase in CI.
13. **1.8** (done): a security review of what 1.7 added (see the
    [security reviews](security-review.md#what-17-added-180)), creates in one round trip, and
    an outbox reader that costs less when idle and isn't slowed by long transactions.
14. **1.9** (done): a health check that shares its database probe for a second and cheaper
    single-record reads; view fields copied from their source collections (and view records'
    files served), as PocketBase does; the VK provider, the last PocketBase preset; the
    differential runner over auth flows, batch requests and error codes; and accessibility and
    screenshot checks of the dashboard in CI.
15. **1.10** (done): the dashboard redesigned as a developer tool: IBM Plex Sans and JetBrains
    Mono, a status bar, neutral GitHub-like colours, flat buttons and far less motion.
16. **1.11** (done): a command center for every project in the main installation, projects
    on other PostgreSQL servers, and an SQL console that only reads unless allowed to write, with
    limits, a schema browser and exports.
17. **2.0** (done): Settings revamped so that every setting does something: appearance with
    named accents, the application's name and images used everywhere, a sign-in page of your
    own, an email designer with a layout and new placeholders, the jobs' history (backups and
    cron jobs), encrypted backups shown as such, storage facts, CORS, maintenance and HTTPS
    only, log settings, import diffs, roles for superusers and API keys.
18. **2.1** (done): lockout after failed sign-ins, the interval between auth emails, file size
    and thumbnail settings and token lifetimes in Settings, and a rate-limit tester.
19. **2.1.1** (done): batches and uploads take turns so the dashboard stays usable during bulk imports,
    and the overview and Health say when request logging is off.
20. **2.1.2** (done): a dashboard whose parts can be told apart (layered surfaces in both themes,
    controls in the accent colour, the mail preview below its editor, a scrolling overview list).
21. **2.1.3** (done): a leaner release pipeline: packages built once, one CI build shared by the
    jobs that run it, the source image built only when its inputs change, and apt and dnf
    repositories that keep the five newest versions (the Pages artifact had outgrown 1 GB).
22. **2.2** (done): the site on its own domain, <https://slonbase.com>, and an installation that
    starts from a bare Linux server: `install.sh` installs PostgreSQL if needed, creates its role
    and database, installs the package and creates the first superuser.
23. **Next:** the differential runner over file uploads and realtime events, and the remaining
    per-request overhead on lists.

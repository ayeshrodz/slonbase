# slonbase design

This document records what slonbase is, the decisions behind it, and the plan. Update it when a
decision changes.

## Goals

- **PocketBase's developer experience on PostgreSQL**: collections defined at runtime, an
  auto-generated REST API, filtering and access rules, auth, realtime, files, an admin dashboard.
- **A single small binary** (currently ~18 MB static, an 8 MB download) with low memory use and
  predictable latency.
- **Rock-solid operation**: safe startup ordering, safe concurrent instances, no data-loss paths,
  clear errors.
- **Easy to install**: Docker Compose, `.deb`/`.rpm` packages, or a plain binary.

## Non-goals

- Bundling PostgreSQL. The database is always external; slonbase connects to it.
- Supporting databases other than PostgreSQL.

## Why a new project instead of a PocketBase fork

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

## PocketBase API compatibility

slonbase aims to match PocketBase's REST API (routes, filter syntax, response and error shapes).
That lets us reuse PocketBase's MIT-licensed admin dashboard and client SDKs at first, and test
slonbase by sending the same requests to both servers and comparing responses. Errors already use
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
- `autogeneratePattern` supports only the `[charset]{n}` form.
- Filters support `geoDistance()` but not yet `strftime()`.

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

**OAuth2.** Presets for Google, GitHub, GitLab, Discord, Microsoft and Facebook, and `oidc`,
`oidc2`, `oidc3` for any OpenID Connect provider (configured URLs). Any preset's URLs can be
overridden (e.g. self-hosted GitLab).
- `auth-methods` lists each provider with a fresh `state` and PKCE pair (`S256`, on unless
  `pkce: false`) and an `authURL` ending in `redirect_uri=` for the client to complete.
- `auth-with-oauth2` exchanges the code (client secret in the form body, 15 s timeout, 1 MiB
  response limit) and fetches the provider's user, then picks the record: the linked one, else
  the signed-in record of the collection (linking a new provider to it), else the one with the
  provider's email, else a new one.
- **Only verified emails are used**, since an email links the provider account to an existing
  record: OIDC/Google need `email_verified`, Discord `verified`, GitHub falls back to the
  verified primary email; Microsoft's `mail` isn't verified, so it's ignored.
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

**Not yet:** OIDC `id_token` validation (the user info endpoint is
used instead), and Apple's and the other PocketBase presets.

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

**Outbox.** Every record create, update and delete through the API also inserts a row into
`_slonbase.events` and calls `pg_notify('slonbase', 'event:<id>')` in the same transaction.
- Rolled-back changes are never announced.
- Notifications arrive in commit order.
- Cascading deletes and the reference updates they cause are announced too.
- Secret fields are stripped from stored events.
- Events are pruned after five minutes.

**Listener.** Every instance holds one dedicated connection that runs `LISTEN slonbase`. For an
event it loads the outbox row and delivers it to its own subscribers, so changes made through
any instance reach every subscriber. The same channel carries `collections` and `settings`
notifications (sent when those change), so all instances reload them. This replaces the
stale-registry limitation of phase 1.

If the connection drops, the listener reconnects with backoff, reloads collections and
settings, and replays outbox rows created since it disconnected. Recently delivered ids are
remembered, so nothing is delivered twice. A test kills the listener's backend mid-stream to
check this.

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

**Not yet:** events for changes made outside the
records API (auth flows that set `verified` or the password, or direct SQL).

### Phase 3: relation paths and expand

**Relation paths** in filters and rules (`author.org.name`) compile to correlated subqueries
over the related tables, never JOINs, so they can't duplicate rows or skew counts.
- If every hop is single-valued, the path is a scalar subquery.
- If any hop or the final field is multi-valued, the path is a set compared with the usual
  all/any (`?`) semantics.
- A missing related record behaves like a single empty value, so `author.name = null` matches
  records without an author.
- Paths can end in a JSON path (`author.meta.key`) and have up to 6 hops.
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
- `fields=...,expand` keeps the expanded records.

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

**Not yet:** deleting the files of a removed file field.

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

**Not yet:** a custom CA for S3 servers with private certificates, multipart uploads (files are
limited to 128 MiB anyway), and presigned URLs.

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

**Not yet:** the Apple client secret generator. (Logs, backups, the Geo Point field, view
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

**Not yet:** `maxDataSize` is accepted but unused (request bodies aren't logged, so entries stay
small anyway).

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
- Counters are in memory, per instance (like PocketBase, which runs as one). A shared limit
  across instances would need a round trip to PostgreSQL per request, which defeats the purpose;
  put a shared limiter in the load balancer if you need one.

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

**Not yet:** encryption at rest. (PocketBase backups aren't restored as backups, but
`slonbase import-pocketbase` takes a PocketBase backup zip.)

### Phase 4e: batch requests

`POST /api/batch` follows PocketBase: `{"requests": [{"method", "url", "headers", "body"}]}` as
JSON, or multipart with the JSON in `@jsonPayload` and files named `requests.{index}.{field}`;
responses are `[{"status", "body"}]`, and a failure answers `400` with
`data.requests.{index}` holding the failing request's error.
- The record handlers' work is split into `create_in`/`update_in`/`delete_in`, which run in a
  caller's transaction; single requests and batches use the same code. File changes are collected
  (`FileEffects`): new files are removed if the transaction fails, replaced and deleted files once
  it commits. Realtime events go to the outbox in the same transaction, so they're delivered only
  if the whole batch commits.
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

### Dashboard redesign

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

### Planned

- **PostgreSQL builds list responses** (`json_agg`/`row_to_json`), so the server streams JSON
  instead of converting rows one by one.

## Configuration

Precedence: flags > environment variables > config file > defaults. Unknown keys in the config
file are rejected, so typos fail loudly. Values from flags or env vars are never written back to
the file.

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
  collections and settings), and catching up after the listener connection is killed.
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
- Benchmarks (`bench/`): the same seeded data and HTTP workloads on slonbase and PocketBase,
  with the server and the load generator on separate CPUs; results and how to read them are in
  [bench/README.md](../bench/README.md).

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

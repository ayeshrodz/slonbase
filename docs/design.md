# slonbase design

This document records what slonbase is, the decisions behind it, and the plan. Update it when a
decision changes.

## Goals

- **PocketBase's developer experience on PostgreSQL**: collections defined at runtime, an
  auto-generated REST API, filtering and access rules, auth, realtime, files, an admin dashboard.
- **A single small binary** (currently ~5 MB static, ~8 MB image) with low memory use and
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

Later phases add `object_store` (local/S3 files), `image` (thumbnails), `lettre` (mail) and an
embedded JS engine (`rquickjs`) for hooks.

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
- User collections (phase 1) will be **real typed tables** in their own schema: `jsonb`,
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
- Request bodies must be JSON.
- Not yet supported:
  - relation paths in filters (`author.name`), `expand` and `@collection`
  - filter functions such as `geoDistance`/`strftime`
  - auth and view collections, file fields

### Planned

- **PostgreSQL builds list responses** (`json_agg`/`row_to_json`), so the server streams JSON
  instead of converting rows one by one.
- **Realtime through `LISTEN/NOTIFY`**, so any number of stateless instances can run behind a
  load balancer and every subscriber sees every change. pgbase propagates only schema and
  settings changes, not realtime events.
- **Backups** with `pg_dump`, or the provider's own backups. pgbase has none.

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
- **Releases**: a `v*` tag must match the `Cargo.toml` version. It publishes a GitHub release
  (packages, tarballs, `SHA256SUMS`) and `ghcr.io/ayeshrodz/slonbase`.
- **Next**: a signed apt repository (`aptly`/`reprepro`, hosted on GitHub Pages or object
  storage) so users get updates with `apt upgrade`. This needs a GPG signing key stored as a
  repository secret.

## Testing

- Unit tests for config, ids, validation and migrations.
- Integration tests against real PostgreSQL (`SLONBASE_TEST_DATABASE_URL`). Each test creates and
  drops its own database. They cover the full wizard flow, reconnecting to an existing install,
  env-configured databases, concurrent migrations, and refusing foreign or newer schemas.
- CI runs them against PostgreSQL 14 (the oldest supported) and 17.
- API tests cover superuser auth, collection CRUD with real schema changes, records with
  filtering/sorting/pagination/modifiers, API rules, relations (validation, cascades, required
  references), and the filter type matrix.
- Planned: running PocketBase's API test scenarios against slonbase to check compatibility, and
  benchmarks against PocketBase.

## Roadmap

0. **Foundation** (done): server, config, setup mode and wizard, system migrations, packaging,
   CI and releases.
1. **Collections** (done): schema management, CRUD, filter/sort/pagination, API rules,
   superuser auth.
2. **Auth**: superusers and auth collections, password and JWT, then OAuth2, OTP and MFA.
3. **Realtime** (`LISTEN/NOTIFY`), **files** (local/S3, thumbnails), relation expansion.
4. **Admin dashboard** (PocketBase's UI first), mail, logs, backups, settings, batch API.
5. **Extensibility**: Rust library API and JS hooks.

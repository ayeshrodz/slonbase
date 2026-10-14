# slonbase

A lightweight, high-performance backend in a single Rust binary, powered by **PostgreSQL**.
Inspired by [PocketBase](https://pocketbase.io): define collections, get a REST API, auth and an
admin dashboard, but on top of your own PostgreSQL server instead of SQLite.

> **Status: early development.** The admin dashboard, collections, the records REST API with
> filtering and API rules, batch requests, auth (superusers and auth collections with password,
> OAuth2 and one-time password sign-in, MFA, verification, password reset and email change),
> realtime subscriptions, relation paths, `expand`, file fields (stored locally or in S3),
> request logs, rate limiting, backups, and JavaScript and Rust hooks work.
> See the [roadmap](#roadmap).

PostgreSQL is **not bundled**. slonbase connects to any PostgreSQL 14+ server, on the same machine,
another machine or a managed service, and sets everything up from a first-run wizard.

## Quick start

### Docker Compose (slonbase + PostgreSQL)

```sh
cp .env.example .env     # set POSTGRES_PASSWORD
docker compose up -d
docker compose logs slonbase | grep "setup?token"
```

Open the printed link to create your first superuser.

### Debian / Ubuntu / Fedora packages

Download the `.deb` or `.rpm` for your architecture from the
[releases](https://github.com/ayeshrodz/slonbase/releases) page, then:

```sh
sudo apt install ./slonbase_<version>_amd64.deb      # or: sudo dnf install ./slonbase-<version>-1.x86_64.rpm
sudo journalctl -u slonbase | grep "setup?token"     # the one-time setup link
```

The package installs a hardened systemd service (`slonbase.service`) running as a dedicated
`slonbase` user, with its config in `/etc/slonbase/config.toml`. An apt repository is planned
so updates arrive through `apt upgrade`.

### Plain binary

```sh
./slonbase serve          # starts in setup mode and prints the setup link
```

## First-run setup

On first start, slonbase has no database configured, so it starts in **setup mode** and prints a
one-time link (the token in it is required, so nobody else reaching the port can take over the
install). The wizard:

1. asks for the PostgreSQL connection and tests it (version, permissions, existing installs),
2. creates the system tables in their own `_slonbase` schema (the database can be shared),
3. creates the first superuser,
4. saves the connection to the config file (`0600` permissions) and switches to normal mode
   without a restart.

When the database URL comes from the environment (as in Docker Compose), the wizard skips step 1.
Pointing a new server at a database that already has a slonbase installation requires signing in
with one of its superusers.

Once set up, slonbase never returns to setup mode: if the database goes down, requests fail with
`503` while it reconnects.

Headless servers can skip the wizard:

```sh
SLONBASE_SUPERUSER_PASSWORD='...' slonbase setup \
    --database-url 'postgres://user:pass@host:5432/db' --email you@example.com
```

## Configuration

Settings come from (highest priority first) command-line flags, environment variables, the config
file, then defaults.

| Flag | Environment variable | Config file | Default |
|---|---|---|---|
| `--config` | `SLONBASE_CONFIG` | | `slonbase.toml` |
| `--bind` | `SLONBASE_BIND` | `server.bind` | `127.0.0.1:8090` |
| `--database-url` | `SLONBASE_DATABASE_URL` | `database.url` | none (setup mode) |
| `--database-max-connections` | `SLONBASE_DATABASE_MAX_CONNECTIONS` | `database.max_connections` | `16` |
| `--database-ca-file` | `SLONBASE_DATABASE_CA_FILE` | `database.ca_file` | none |
| `--storage-path` | `SLONBASE_STORAGE_PATH` | `storage.path` | `slonbase_data/storage` |
| `--backups-path` | `SLONBASE_BACKUPS_PATH` | `backups.path` | `slonbase_data/backups` |
| `--sql-console` | `SLONBASE_SQL_CONSOLE` | `server.sql_console` | off |
| `--hooks-dir` | `SLONBASE_HOOKS_DIR` | `hooks.dir` | `slonbase_hooks` |
| `--hooks-pool` | `SLONBASE_HOOKS_POOL` | `hooks.pool_size` | `10` |
| `serve --setup-token` | `SLONBASE_SETUP_TOKEN` | | random |

Database URLs use the standard libpq format, e.g.
`postgres://user:pass@host:5432/db?sslmode=require`. TLS uses the Mozilla root certificates, so
add `--database-ca-file` for providers with their own CA (such as Amazon RDS).

Logging is controlled by `RUST_LOG` (default `info`).

## Admin dashboard

The dashboard is at `/_/` (e.g. `http://localhost:8090/_/`): slonbase's own UI, built into the
binary. It covers:
- an overview of your collections and traffic;
- the data workspace (live record grid, record editor with uploads and relation pickers, schema
  editor, API rules, sign-in options, indexes, and API docs for every collection);
- logs, the SQL console, and settings (mail, file storage, backups, rate limits, superuser IPs,
  cron jobs, import and export).

Press ⌘K / Ctrl K anywhere to jump to a collection or action. The design is deliberately quiet:
neutral surfaces and one muted accent (changeable in the settings), in a light or dark theme
that follows your system, and it works on phones. It uses
only the public API, so anything it does you can script too; see [ui/README.md](ui/README.md)
to work on it.

**Logs.** Every request is logged (level 0, or 8 for errors, with the error), except dashboard
files and successful health checks, and so are slonbase's own warnings and errors. They're kept
for `logs.maxDays` days (5 by default, 0 turns logging off) in an unlogged PostgreSQL table, so
writing them is cheap, and they're written in batches in the background. Client IPs come from the
connection, or from the headers in `trustedProxy` when you run behind a proxy.

**Rate limits** (`rateLimits`, off by default) follow PocketBase's rules: a label is
`{collection}:{action}` or `*:{action}` (`list`, `view`, `create`, `update`, `delete`, `auth`,
`authWithPassword`, `requestOTP`, `file`, ...), `METHOD /path`, or a path prefix ending in `/`,
optionally only for `@guest` or `@auth` requests. Each allows `maxRequests` per `duration`
seconds per client IP; over the limit, requests get `429`. Superusers and `excludedIPs` aren't
limited. Counters are kept in memory by each instance, so with several instances the limits apply
per instance.

**Batch requests** (`POST /api/batch`, `pb.createBatch()` in the SDKs) run record creates,
updates, upserts and deletes in one transaction: all of them apply, or none (with the failing
request's error). Enable them with `batch.enabled`; `maxRequests`, `timeout` (seconds) and
`maxBodySize` limit them. Each request uses the batch's auth and rules; `expand` in a request's URL
shows the related records as of the end of the batch.

**Backups** (`/api/backups`, or the dashboard's Backups page) are zip files with every collection,
the settings, and the local files (unless files are in S3). Restoring one replaces all of that in
one transaction, on every instance; it also restores the installation's signing key, so tokens
issued after the backup stop working. Backups are kept in `--backups-path`, or in an S3 bucket
(`backups.s3`); `backups.cron` (e.g. `0 3 * * *`, UTC) creates them automatically and
`backups.cronMaxKeep` limits how many are kept. They're slonbase's own format (not `pg_dump`,
which the Docker image doesn't have), so restore them with slonbase; you can still back up the
database with your usual PostgreSQL tools.

**Superuser IPs** (`superuserIPs`): when set, superuser requests, superuser sign-in and the
`_superusers` endpoints only work from those addresses or ranges. Behind a proxy, set
`trustedProxy` first, or slonbase sees the proxy's address.

The **SQL console** runs any SQL a superuser enters with the database role's privileges. With
PostgreSQL that can reach outside slonbase's data (a superuser role can read server files or run
programs with `COPY ... PROGRAM`), so it's off unless you start slonbase with `--sql-console`.
Queries starting with `SELECT`, `WITH`, `SHOW`, `EXPLAIN`, `VALUES` or `TABLE` run read-only;
everything has a 3 minute timeout.

## Commands

| Command | Description |
|---|---|
| `slonbase serve` | Start the server (default). |
| `slonbase setup` | Set up without the web wizard. |
| `slonbase health` | Exit 0 if the local server is healthy (used by the Docker health check). |

## API

The API follows PocketBase's, so its docs and SDKs mostly apply. Supported so far:

| Endpoint | Description |
|---|---|
| `POST /api/collections/{collection}/auth-with-password` | Sign in (`{"identity", "password"}`), returns `{token, record}`; use `_superusers` for superusers |
| `POST /api/collections/{collection}/auth-with-oauth2` | Sign in with an OAuth2 provider (`{"provider", "code", "codeVerifier", "redirectURL", "createData"}`) |
| `POST /api/collections/{collection}/request-otp`, `auth-with-otp` | Sign in with a one-time password sent by email |
| `GET/POST /api/oauth2-redirect` | OAuth2 redirect for the SDKs' popup flow (`authWithOAuth2({provider})`) |
| `GET /api/collections/{collection}/records/{id}/external-auths`, `DELETE .../{provider}` | Linked OAuth2 accounts |
| `POST /api/collections/{collection}/auth-refresh` | A new token for a valid one |
| `GET /api/collections/{collection}/auth-methods` | Available sign-in methods |
| `POST /api/collections/{collection}/request-verification`, `confirm-verification` | Email verification |
| `POST /api/collections/{collection}/request-password-reset`, `confirm-password-reset` | Password reset by email |
| `POST /api/collections/{collection}/request-email-change`, `confirm-email-change` | Email change (signed in) |
| `POST /api/collections/{collection}/impersonate/{id}` | A non-refreshable token for any record (superusers) |
| `GET/PATCH /api/settings` | Settings (superusers): app, SMTP, S3, backups, logs, batch, rate limits, trusted proxy, superuser IPs |
| `POST /api/settings/test/email` | Send a test email of a template |
| `POST /api/settings/test/s3` | Check the S3 settings by writing, reading and deleting a test file |
| `GET /api/files/{collection}/{record}/{filename}` | Download a file (`thumb=100x100`, `download=1`, `token=` for protected files) |
| `POST /api/files/token` | A short-lived token for protected files |
| `GET /api/realtime`, `POST /api/realtime` | Realtime: an SSE stream, and setting its subscriptions (`posts/*`, `posts/{id}`) |
| `GET/POST /api/collections`, `GET/PATCH/DELETE /api/collections/{idOrName}` | Manage collections (superusers) |
| `PUT /api/collections/import` | Create, update (and with `deleteMissing`, delete) collections in one transaction |
| `DELETE /api/collections/{collection}/truncate` | Delete every record of a collection |
| `GET /api/collections/meta/scaffolds`, `meta/oauth2-providers` | Defaults and providers for the dashboard |
| `POST /api/sql` | The SQL console (when enabled) |
| `GET/DELETE /api/logs`, `GET /api/logs/{id}`, `GET /api/logs/stats` | Logs (filter, sort, per-hour stats), superusers |
| `GET/POST /api/backups`, `POST /api/backups/upload`, `GET/DELETE /api/backups/{key}`, `POST /api/backups/{key}/restore` | Backups (download with a superuser file token) |
| `GET /api/crons`, `POST /api/crons/{id}` | Scheduled jobs (the automatic backup) |
| `POST /api/batch` | Several record creates, updates, upserts (`PUT`) and deletes in one transaction (enable `batch` in the settings) |
| `GET/POST /api/collections/{collection}/records` | List (`page`, `perPage`, `sort`, `filter`, `expand`, `fields`, `skipTotal`) and create records |
| `GET/PATCH/DELETE /api/collections/{collection}/records/{id}` | View, update and delete a record |
| `GET /api/health` | Health check |

Send the token as `Authorization: Bearer <token>` (or just the token).

```sh
# create a collection anyone can read and add to
curl -X POST localhost:8090/api/collections -H "Authorization: $TOKEN" -H 'content-type: application/json' \
  -d '{"name": "todos", "fields": [{"name": "text", "type": "text", "required": true},
       {"name": "done", "type": "bool"}], "listRule": "", "createRule": ""}'

curl -X POST localhost:8090/api/collections/todos/records -H 'content-type: application/json' -d '{"text": "buy milk"}'
curl "localhost:8090/api/collections/todos/records?filter=done=false&sort=-text"
```

**System collections:** superusers are records of the `_superusers` auth collection and linked
OAuth2 accounts are records of `_externalAuths`, as in PocketBase, so both are managed with the
records API. System collections can't be renamed or deleted, and the last superuser can't be
deleted.

**Collections** are `base` or `auth`. Auth collections add the `password`, `tokenKey`, `email`,
`emailVisibility` and `verified` system fields and PocketBase's auth options (`authRule`,
`manageRule`, `passwordAuth`, `oauth2`, `otp`, `mfa`, token durations, email templates). Sign up
by creating a record with `password` and `passwordConfirm`, or with OAuth2.

**Sign-in methods:**
- **OAuth2**: Google, GitHub, GitLab, Discord, Microsoft, Facebook, and any OpenID Connect
  provider (`oidc`, `oidc2`, `oidc3` with your URLs), with PKCE. Only emails the provider has
  verified are used to find existing records. Client secrets are never returned by the API.
- **OTP**: a numeric code sent with the `otp.emailTemplate` (`{OTP}`, `{OTP_ID}`); single-use,
  expiring, and invalidated after 5 wrong attempts.
- **MFA**: with `mfa.enabled`, the first sign-in answers `401 {"mfaId"}`, and a sign-in with
  another method plus that `mfaId` returns the token. `mfa.rule` limits it to some records.

**Field types:** `text`, `editor`, `number`, `bool`, `email`, `url`, `date`, `autodate`, `select`,
`json`, `relation`, `file` and `password` (auth collections).

**Files** are uploaded with `multipart/form-data` on create and update:
- `avatar` replaces a field's files, `files+` appends, and `files-` (or a list of kept names)
  removes.
- Other fields are sent as form values, or as JSON in `@jsonPayload`.
- Types are detected from the content (never the name) and checked against `mimeTypes`.
- Thumbnails (`100x100`, `0x300`, `200x100t`/`b`/`f`) are generated on first request.
- Files are stored under `--storage-path`, or in an S3 bucket (see below).

**S3 storage.** Enable it in the settings, like in PocketBase, and every instance switches to the
bucket (useful when you run several instances behind a load balancer):

```sh
curl -X PATCH localhost:8090/api/settings -H "Authorization: $TOKEN" -H 'content-type: application/json' \
  -d '{"s3": {"enabled": true, "bucket": "my-files", "region": "eu-west-1",
       "endpoint": "https://s3.eu-west-1.amazonaws.com", "accessKey": "...", "secret": "...",
       "forcePathStyle": false}}'
curl -X POST localhost:8090/api/settings/test/s3 -H "Authorization: $TOKEN" -d '{}'   # 204 when it works
```

Any S3-compatible service works (MinIO, Cloudflare R2, Backblaze B2, Wasabi, DigitalOcean Spaces;
use `forcePathStyle: true` for MinIO and other servers addressed by IP or a single host name). The
secret is never returned by the API. Existing files aren't moved when you switch storage.

**Collections** are real PostgreSQL tables in the `slonbase` schema (`TEXT`,
`DOUBLE PRECISION`, `BOOLEAN`, `TIMESTAMPTZ`, `JSONB`), so other tools can query them directly.

**Filters and rules** use PocketBase's syntax: `=`, `!=`, `>`, `>=`, `<`, `<=`, `~` (contains),
`!~`, the `?`-prefixed "any of" variants for multi-valued fields, `&&`, `||`, parentheses,
relation paths (`author.org.name`, `tags.name ?= 'x'`), `@request.*` (including
`@request.auth.*`, the signed-in record's fields), date macros such as `@now`, JSON paths (`meta.key`) and the `:length`, `:lower`,
`:each` and `:isset` modifiers. A rule of `null` allows superusers only, `""` allows anyone.

**Realtime** follows PocketBase's protocol, so `pb.collection('posts').subscribe('*', ...)` works.
Events respect the collection's list/view rules for each subscriber. Changes are announced
through PostgreSQL `LISTEN/NOTIFY`, so you can run several slonbase instances on one database
behind a load balancer: realtime events, collection changes and settings reach all of them.

Emails (verification, password reset, email change) are sent through the SMTP server in the
settings; while SMTP is disabled they're written to the log.

**New device alerts** (`authAlert`, on by default) email a record when it signs in from an IP
address and user agent it hasn't used before (not on its first sign-in, nor the first after a
password change). The last 5 devices per record are remembered.

**Expand** loads related records into each record's `expand` key: relation fields
(`expand=author.org,editors`) and back-relations (`expand=comments_via_post`), up to 6 levels,
each checked against the related collection's rules.

**Geo Point** fields store `{"lon": ..., "lat": ...}`; filters can use `loc.lat` / `loc.lon` and
`geoDistance(lonA, latA, lonB, latB)` (kilometers), e.g.
`geoDistance(loc.lon, loc.lat, 23.32, 42.69) < 25`.

Rules and filters can look at other collections with `@collection.name[:alias].field`, e.g.
`@collection.members.post = id && @collection.members.user = @request.auth.id` (true when some
row of `members` matches; each alias is a separate row, up to 6 per expression).

**View collections** are read-only records from a SQL query, e.g.
`SELECT author AS id, count(*) AS posts FROM posts GROUP BY author` (collections are tables by
their names). Their fields come from the query's columns, and list/view rules, filters, sorting
and expand work as usual. Because the query runs with the database role's privileges (a
`SELECT` can call any function the role may use), creating or changing one needs the SQL
console to be enabled (`--sql-console`); existing views keep working without it.

Not supported yet (see [docs/design.md](docs/design.md)): request hooks inside batch requests.

## Extending slonbase

### JavaScript hooks

Put `*.pb.js` files in the hooks directory (`slonbase_hooks`; `/data/hooks` in Docker,
`/var/lib/slonbase/hooks` for the packages) and restart. They use PocketBase's JavaScript API, so
most PocketBase hooks work as they are:

```js
// slonbase_hooks/main.pb.js
onRecordCreateRequest((e) => {
  e.record.set("status", "draft");
  e.next(); // validates and saves the record
}, "posts");

onRecordAfterCreateSuccess((e) => {
  console.log("new post", e.record.id);
  e.next();
}, "posts");

routerAdd("GET", "/api/hello/{name}", (e) => {
  const posts = $app.findRecordsByFilter("posts", "status = {:s}", "-created", 10, 0, { s: "draft" });
  return e.json(200, { hello: e.request.pathValue("name"), drafts: posts.length });
}, $apis.requireAuth());

cronAdd("cleanup", "0 3 * * *", () => {
  $app.db().newQuery("DELETE FROM posts WHERE status = {:s}").bind({ s: "spam" }).execute();
});
```

Available: `onRecord{Create,Update,Delete,View}Request`, `onRecordsListRequest`,
`onRecordAfter{Create,Update,Delete}Success`, `onBootstrap`, `routerAdd` (with `$apis.requireAuth`,
`requireSuperuserAuth`, `requireSuperuserOrOwnerAuth` and `requireGuestOnly`), `cronAdd`/`cronRemove`,
`Record` (`get`/`set`/`getString`/..., `setPassword`, `validatePassword`, `newAuthToken`), `$app`
(`findRecordById`, `findRecordsByFilter`, `findFirstRecordByFilter`, `findAuthRecordByEmail`,
`countRecords`, `findCollectionByNameOrId`, `save`, `delete`, `db().newQuery(...)`, `settings`,
`newMailClient`), `$http.send`, `$security` (`randomString`, `sha256`, `hs256`, ...), `$os.getenv`,
`require()` for modules in the hooks directory, and `console`. Errors thrown as `BadRequestError`,
`NotFoundError`, `ForbiddenError`, `UnauthorizedError` or `ApiError` become API errors; other errors
answer 400 and are logged.

Each of the `--hooks-pool` runtimes loads every file, so top-level code runs once per runtime; keep
state in the database rather than in variables. Not supported (yet): model hooks
(`onRecordCreate`, ...) and other events, `routerUse`, file uploads in custom routes,
`$filesystem`, and reloading when the files change. Build with `--no-default-features` to leave
the JavaScript engine out.

### Rust

slonbase is also a Rust library. To add hooks, routes and cron jobs, depend on the `slonbase`
crate and run its command line from your own `main`:

```rust
use axum::routing::get;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let mut ext = slonbase::Extensions::default();
    // runs before the record is written; `next` writes it
    ext.hooks.on_record_create_request.add(&["posts"], |mut e, next| async move {
        e.record.insert("status".into(), "draft".into());
        next.run(e).await
    });
    // runs after the change is committed (API writes, batches and the record service)
    ext.hooks.on_record_after_create_success.add(&["posts"], |e, next| async move {
        tracing::info!(id = %e.record["id"], "new post");
        next.run(e).await
    });
    ext.hooks.cron("nightly", "0 3 * * *", |core| async move {
        let old = core.count_records("posts", "created < {:t}", &serde_json::Map::from_iter([
            ("t".into(), "2025-01-01".into()),
        ])).await?;
        tracing::info!(old, "old posts");
        Ok(())
    })
    .expect("a valid cron expression");
    ext.route("/api/hello", get(|| async { "Hello!" }));
    slonbase::cli::run(ext).await
}
```

Hooks: `on_record_{create,update,delete,view}_request`, `on_records_list_request` (around API
requests; a hook can change the record, return an error, or set `e.response` instead of calling
`next`) and `on_record_after_{create,update,delete}_success` (after commit, in the background).
`hooks.on_ready` runs when the app is ready. Cron jobs run on one instance per scheduled minute
and show up in the dashboard. Hooks, routes and cron jobs can read and write records with
`core.find_record_by_id`, `find_records_by_filter`, `find_first_record_by_filter`,
`count_records`, `save_record` and `delete_record` (no API rules; filters take `{:name}`
placeholders).

## Development

```sh
cargo test                     # unit tests
# integration tests need a PostgreSQL role that can create databases:
SLONBASE_TEST_DATABASE_URL='postgres://postgres:postgres@127.0.0.1:5432/postgres?sslmode=disable' cargo test
# S3 tests also need an S3-compatible server (path-style), e.g. MinIO:
SLONBASE_TEST_S3_ENDPOINT=http://127.0.0.1:9000 SLONBASE_TEST_S3_ACCESS_KEY=... SLONBASE_TEST_S3_SECRET=... cargo test
```

Packages are built with [nfpm](https://nfpm.goreleaser.com):

```sh
cargo build --release --locked --target x86_64-unknown-linux-musl
scripts/package.sh x86_64-unknown-linux-musl amd64 0.1.0     # → dist/
```

Pushing a `v*` tag publishes a GitHub release with `.deb`, `.rpm` and `.tar.gz` files for amd64
and arm64, plus a multi-arch image at `ghcr.io/ayeshrodz/slonbase`.

The design and the reasoning behind it are in [docs/design.md](docs/design.md).

## Roadmap

0. **Foundation** (done): server, config, first-run setup, system migrations, packaging, CI.
1. **Collections** (done): schema management, CRUD, filtering/sorting/pagination, API rules.
2. **Auth** (done): superusers and auth collections with password, OAuth2 and OTP sign-in, MFA,
   verification, password reset and email change.
3. **Realtime, relation expansion and files** (done), with local or S3 storage.
4. **Admin dashboard**, **logs**, **rate limiting**, **backups** and **batch requests** (done).
5. **Extensibility** (done): JavaScript hooks with PocketBase's API, and a Rust library API.

## License

[MIT](LICENSE). The dashboard vendors Preact (MIT) and htm (Apache-2.0); their licenses are in
[ui/app/vendor](ui/app/vendor).
slonbase is an independent project inspired by PocketBase (also MIT) and is not
affiliated with it.

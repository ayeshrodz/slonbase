# slonbase

A lightweight, high-performance backend in a single Rust binary, powered by **PostgreSQL**.
Inspired by [PocketBase](https://pocketbase.io): define collections, get a REST API, auth and an
admin dashboard, but on top of your own PostgreSQL server instead of SQLite.

> **Status: early development (phase 3).** Collections, the records REST API with filtering and
> API rules, auth (superusers and auth collections with password, OAuth2 and one-time password
> sign-in, MFA, verification, password reset and email change), realtime subscriptions, relation
> paths, `expand` and file fields work. S3 storage and the admin dashboard are next.
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
| `serve --setup-token` | `SLONBASE_SETUP_TOKEN` | | random |

Database URLs use the standard libpq format, e.g.
`postgres://user:pass@host:5432/db?sslmode=require`. TLS uses the Mozilla root certificates, so
add `--database-ca-file` for providers with their own CA (such as Amazon RDS).

Logging is controlled by `RUST_LOG` (default `info`).

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
| `GET/PATCH /api/settings` | App name/URL and SMTP settings (superusers) |
| `GET /api/files/{collection}/{record}/{filename}` | Download a file (`thumb=100x100`, `download=1`, `token=` for protected files) |
| `POST /api/files/token` | A short-lived token for protected files |
| `GET /api/realtime`, `POST /api/realtime` | Realtime: an SSE stream, and setting its subscriptions (`posts/*`, `posts/{id}`) |
| `GET/POST /api/collections`, `GET/PATCH/DELETE /api/collections/{idOrName}` | Manage collections (superusers) |
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
- Files are stored under `--storage-path`. Several instances need a shared directory until S3
  support lands. Each collection is a real PostgreSQL table in the `slonbase` schema
(`TEXT`, `DOUBLE PRECISION`, `BOOLEAN`, `TIMESTAMPTZ`, `JSONB`), so other tools can query it directly.

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

**Expand** loads related records into each record's `expand` key: relation fields
(`expand=author.org,editors`) and back-relations (`expand=comments_via_post`), up to 6 levels,
each checked against the related collection's rules.

Not supported yet (see [docs/design.md](docs/design.md)): auth alerts, `@collection` lookups,
filter functions, S3 storage, and view collections.

## Development

```sh
cargo test                     # unit tests
# integration tests need a PostgreSQL role that can create databases:
SLONBASE_TEST_DATABASE_URL='postgres://postgres:postgres@127.0.0.1:5432/postgres?sslmode=disable' cargo test
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
3. **Realtime, relation expansion and files** (done). S3 storage follows.
4. **Admin dashboard**, mail, logs, backups, settings, batch requests.
5. **Extensibility**: use as a Rust library, and JS hooks.

## License

[MIT](LICENSE). slonbase is an independent project inspired by PocketBase (also MIT) and is not
affiliated with it.

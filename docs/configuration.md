# Configuration

slonbase has two kinds of configuration:

- **Server configuration** covers what the process needs before it can reach the database:
  where to listen, the database connection, and local paths. It comes from flags, environment
  variables and a TOML file.
- **Application settings** are stored in the database and shared by every instance: app name
  and URL, mail, S3, backups, logs, rate limits and so on. You change them in the dashboard
  (Settings) or with `PATCH /api/settings`, and they apply right away, without a restart.

## Server configuration

Values come from (highest priority first) command-line flags, environment variables, the config
file, and then the defaults.

| Flag | Environment variable | Config file | Default |
|---|---|---|---|
| `--config` | `SLONBASE_CONFIG` | | `slonbase.toml` |
| `--bind` | `SLONBASE_BIND` | `server.bind` | `127.0.0.1:8090` |
| `--metrics-bind` | `SLONBASE_METRICS_BIND` | `server.metrics_bind` | none (no metrics) |
| `--http-ca-file` | `SLONBASE_HTTP_CA_FILE` | `server.http_ca_file` | none |
| `--database-url` | `SLONBASE_DATABASE_URL` | `database.url` | none (setup mode) |
| `--database-max-connections` | `SLONBASE_DATABASE_MAX_CONNECTIONS` | `database.max_connections` | `16` |
| `--database-ca-file` | `SLONBASE_DATABASE_CA_FILE` | `database.ca_file` | none |
| `--storage-path` | `SLONBASE_STORAGE_PATH` | `storage.path` | `slonbase_data/storage` |
| `--backups-path` | `SLONBASE_BACKUPS_PATH` | `backups.path` | `slonbase_data/backups` |
| `--backups-encryption-key` | `SLONBASE_BACKUPS_ENCRYPTION_KEY` | `backups.encryption_key` | none (not encrypted) |
| `--encryption-key` | `SLONBASE_ENCRYPTION_KEY` | `server.encryption_key` | none (settings not encrypted) |
| `--sql-console` | `SLONBASE_SQL_CONSOLE` | `server.sql_console` | off |
| `--sql-console-writes` | `SLONBASE_SQL_CONSOLE_WRITES` | `server.sql_console_writes` | off (the console only reads) |
| `--no-compression` | `SLONBASE_NO_COMPRESSION` | `server.compression` | on ([gzip for JSON](api.md#compression)) |
| `--hooks-dir` | `SLONBASE_HOOKS_DIR` | `hooks.dir` | `slonbase_hooks` |
| `--hooks-pool` | `SLONBASE_HOOKS_POOL` | `hooks.pool_size` | `10` |
| `--hooks-watch` | `SLONBASE_HOOKS_WATCH` | `hooks.watch` | `true` |
| `--migrations-dir` | `SLONBASE_MIGRATIONS_DIR` | `migrations.dir` | `slonbase_migrations` |
| `--automigrate` | `SLONBASE_AUTOMIGRATE` | `migrations.automigrate` | `false` |
| `--projects-path` | `SLONBASE_PROJECTS_PATH` | `projects.path` | `slonbase_data/projects` |
| | | `projects.max_connections` | `4` |
| `serve --setup-token` | `SLONBASE_SETUP_TOKEN` | | random |

Relative paths are relative to the working directory. The Docker image sets
`SLONBASE_CONFIG=/data/slonbase.toml` and keeps the paths under `/data`. The packages pass
`--config /etc/slonbase/config.toml` and paths under `/var/lib/slonbase` in the service file.

A config file with every option:

```toml
[server]
bind = "127.0.0.1:8090"
sql_console = false
# sql_console_writes = false   # let the SQL console change data (INSERT, UPDATE, DELETE, DDL)
# compression = false   # don't gzip JSON answers (saves their CPU where bandwidth is free)
# metrics_bind = "127.0.0.1:9090"   # Prometheus metrics at /metrics (not authenticated)
# http_ca_file = "/etc/slonbase/private-ca.pem"   # extra CAs for S3 and OAuth2 providers
# encryption_key = "..."   # encrypts the settings in the database; prefer SLONBASE_ENCRYPTION_KEY

[database]
url = "postgres://slonbase:secret@127.0.0.1:5432/slonbase"
max_connections = 16
# ca_file = "/etc/ssl/certs/rds-global-bundle.pem"

[storage]
path = "slonbase_data/storage"

[backups]
path = "slonbase_data/backups"
# encryption_key = "..."   # encrypts new backups; prefer SLONBASE_BACKUPS_ENCRYPTION_KEY

[hooks]
dir = "slonbase_hooks"
pool_size = 10
watch = true

[migrations]          # see migrations.md
dir = "slonbase_migrations"
automigrate = false

[projects]            # see projects.md
path = "slonbase_data/projects"
max_connections = 4
```

The setup wizard writes `database.url` into this file with `0600` permissions, since it contains
the password. When the URL comes from a flag or the environment, the wizard doesn't change it.

**Database URLs** use the libpq format, e.g.
`postgres://user:pass@host:5432/db?sslmode=require`. Percent-encode special characters in the
password. `sslmode` can be `disable`, `prefer` (the default) or `require`. TLS checks the server's
certificate against the Mozilla root certificates; for providers with their own CA (such as
Amazon RDS), pass it with `--database-ca-file`.

**Connections:** each instance opens up to `max_connections` to PostgreSQL, plus two for
`LISTEN` (realtime and projects), and each [project](projects.md) adds its own. With several instances, keep the total below the server's `max_connections`, or use a
pooler in session mode. Transaction-mode poolers (PgBouncer `pool_mode = transaction`) don't
support `LISTEN`, so slonbase needs a direct or session-mode connection.

**Logging** goes to stderr and is controlled by `RUST_LOG` (default `info`; `warn`, `debug`, and
per-module filters such as `info,slonbase=debug` also work). Requests are logged to the
database too; see [logs](#logs).

**SQL console** (`--sql-console`): the dashboard's SQL page, for superusers. See
[security](deployment.md#security-checklist) before you turn it on.

- **Read-only by default.** It runs one statement at a time, and only reads: `SELECT`, `WITH`,
  `SHOW`, `EXPLAIN`, `VALUES` and `TABLE`, in a `READ ONLY` transaction. PostgreSQL itself
  refuses anything that would write, including a `WITH ... DELETE` or a function that writes.
- **Writes** (`--sql-console-writes`, `SLONBASE_SQL_CONSOLE_WRITES=true`) let it run any
  statement or script (`INSERT`, `UPDATE`, `DELETE`, DDL), in one transaction that commits. Turn
  it on where you need it, not everywhere.
- **Limits:** rows come through a cursor, 1,000 by default and at most 5,000 a query (16 MB of
  values); statements stop after 30 seconds, or 5 seconds waiting for a lock; queries are at most
  20,000 characters; two run at a time per instance. Each query gets a connection of its own,
  closed afterwards, so nothing it sets outlives it.
- Collections are tables in the `slonbase` schema, which the console searches first:
  `SELECT * FROM tasks` works as written.
- Results can be exported from the dashboard as Excel (`.xlsx`), CSV, TSV or JSON: the rows the
  query returned, built in the browser.

### Encrypting the settings

The database holds secrets: the settings' (the SMTP password, the S3 secret keys), the secret
every token is signed with (whoever reads it can sign in as anyone), the OAuth2 providers'
client secrets in auth collections, and the passwords of [projects](projects.md)' own database
roles. With an encryption key, all of them are stored encrypted (AES-256-GCM), so a database
dump, a read-only database role or a backup gives nothing away without the key.

```sh
export SLONBASE_ENCRYPTION_KEY="$(openssl rand -hex 16)"   # 32 characters; keep it safe
slonbase serve
```

- **Existing installations** are encrypted on the first start with a key (the settings, the
  signing secret, client secrets and project role passwords); tokens stay valid.
- **Every instance**, and the `slonbase` commands you run against the database, need the same
  key. Projects use the main installation's.
- **Without the key, or with another one,** slonbase doesn't start, and says why. A lost key
  can't be recovered: keep it in a password manager or your secrets store, apart from the
  database and its backups.
- **Backups** carry them encrypted, and restore only where the key is the same; a restore that
  can't read them is refused and changes nothing. Restoring a backup made without a key
  encrypts its settings again.
- **Changing or removing the key:** stop every instance, run
  `slonbase rekey --to <new key>` (or `slonbase rekey` with no `--to`, to store them
  unencrypted) with the current key set as usual, then start the instances with the new one.
  It covers the main installation and every project.
- **PocketBase:** the format is `PB_ENCRYPTION`'s, so with the same value as the key, an import
  brings PocketBase's encrypted settings along.

Records themselves aren't encrypted: protect them with the database's own access control, and
encrypted disks and backups.

## Application settings

These are the sections of `GET/PATCH /api/settings`, and of the dashboard's Settings pages.

| Section | What it controls |
|---|---|
| `meta` | `appName` (the browser tab, the sidebar, the sign-in page, emails), `appURL` (email links, OAuth2 redirects), `senderName`, `senderAddress`, `accentColor` (a named accent such as `ocean`, or `#rrggbb`), `hideControls`, and the images `logo`, `logoDark`, `favicon` (ids from Settings → Branding) |
| `smtp` | Outgoing mail: `enabled`, `host`, `port`, `username`, `password`, `authMethod`, `tls`, `localName`. While it's disabled, emails are written to the log. |
| `s3` | File storage in an S3 bucket instead of `--storage-path` |
| `backups` | `cron` (automatic backups), `cronMaxKeep`, and `s3` (keep backups in a bucket) |
| `logs` | `maxDays` (0 turns request logs off), `minLevel`, `logIP`, `logAuthId`, and slonbase's `excludePaths`, `slowRequestMs`, `maxRows` |
| `batch` | `enabled`, `maxRequests`, `timeout`, `maxBodySize` for `/api/batch` |
| `rateLimits` | `enabled`, `rules` |
| `trustedProxy` | `headers` carrying the client IP, `useLeftmostIP` |
| `superuserIPs` | Addresses or ranges superusers may use |
| `security` | slonbase's: `allowedOrigins` (CORS; `["*"]` by default), `maxUploadMb` (128), `hsts`, `maintenance`, `maintenanceMessage` |
| `login` | slonbase's: the sign-in page's `layout` (`split`, `centered`, `minimal`), `background` (`none`, `grid`, `asset`) and `backgroundAsset`, `title`, `subtitle`, `logo`, `notice`, `footerLinks` (up to 3, `https:` or `mailto:`), `showPoweredBy` |
| `mailLayout` | slonbase's: `enabled`, `html` (a document with `{CONTENT}` where each email's body goes) |

Roles and API keys are kept with the settings but only `/api/team` shows and changes them.

Emails fill `{APP_NAME}`, `{APP_URL}`, `{TOKEN}` (and `{OTP}`, `{OTP_ID}`, `{ALERT_INFO}` in their
templates) as PocketBase does, and slonbase's `{APP_LOGO}` (the logo, or the name),
`{APP_LOGO_URL}`, `{ACCENT}` and `{RECORD:field}` (the recipient's fields, escaped; hidden
fields stay empty).

**CORS.** Pages on any site may call the API, as with PocketBase (`security.allowedOrigins` is
`["*"]`). List origins (`https://app.example.com`) to allow only those; an empty list allows
none, so browsers only call the API from its own address. **Maintenance** answers `503` with
your message to everyone but superusers and `/api/health`.

Secrets (the SMTP password, S3 secrets, OAuth2 client secrets) are never returned by the API.
`POST /api/settings/test/email` and `POST /api/settings/test/s3` check the mail and S3 settings.

### File storage in S3

When `s3.enabled` is set, every instance stores new files in the bucket. You need this when you
run several instances behind a load balancer.

```sh
curl -X PATCH localhost:8090/api/settings -H "Authorization: $TOKEN" -H 'content-type: application/json' \
  -d '{"s3": {"enabled": true, "bucket": "my-files", "region": "eu-west-1",
       "endpoint": "https://s3.eu-west-1.amazonaws.com", "accessKey": "...", "secret": "...",
       "forcePathStyle": false}}'
curl -X POST localhost:8090/api/settings/test/s3 -H "Authorization: $TOKEN" -d '{}'   # 204 when it works
```

Any S3-compatible service works: AWS, MinIO, Cloudflare R2, Backblaze B2, Wasabi or DigitalOcean
Spaces. Use `forcePathStyle: true` for MinIO and other servers addressed by IP or a single host
name. Existing files aren't moved when you switch storage.

An S3 server with a certificate from a private CA (a MinIO on your network, say) needs that CA:
pass its PEM file with `--http-ca-file`. It's trusted besides the usual public roots, for all
outbound HTTPS (S3, and OAuth2 providers on the same network).

**Downloads straight from the bucket.** With `s3.presignedDownloads` on, a file request
gets a redirect to a short-lived signed URL of the bucket instead of the file itself, so the
bytes don't pass through slonbase. The checks stay in slonbase: the file must exist, a protected
file still needs a valid file token, thumbnails are made first, and the download hook runs. The
signed URL makes the bucket answer with the type, disposition and caching slonbase would have
sent. Public files get a URL that stays the same for an hour, so browsers cache them; a protected
file's URL lasts five minutes at most and isn't cached. Clients must be able to reach the S3
endpoint, so this suits a public endpoint (AWS, R2, ...) more than a MinIO on an internal
network. It's off by default, and backups ignore it.

Large files (backups, mostly) are uploaded in 16 MiB parts above 64 MiB, so each part is
retried on its own and objects can pass the 5 GiB a single upload allows. A failed upload is
aborted, so no parts are left behind.

### Logs

Every request is logged at level 0, or level 8 with the error for failed requests. Dashboard
files and successful health checks are skipped. slonbase's own warnings and errors are logged
too. Logs are kept for `logs.maxDays` days (5 by default) in an unlogged PostgreSQL table, so
writing them is cheap. They're written in batches in the background, and they don't survive a
PostgreSQL crash. Browse them in the dashboard or with `/api/logs`. An entry's data is kept
under `logs.maxDataSize` bytes (16 KiB when 0): a larger one keeps the attributes that fit and
is marked `"__pb_truncated__": true`. Paths in `logs.excludePaths` aren't logged unless they
fail with a server error; requests slower than `logs.slowRequestMs` are logged as warnings (and
marked `"slow": true`) whatever `minLevel`; `logs.maxRows` caps the entries kept.

### Rate limits

`rateLimits` limits requests per client IP. New installations (and new projects) start with them
on and these rules: `*:auth` 2 requests per 3 seconds, `*:create` 20 per 5 seconds,
`/api/batch` 3 per second and `/api/` 300 per 10 seconds. Installations from before 0.4.2, and
imported ones, start with them off. A rule's `label` is one of:

- `{collection}:{action}` or `*:{action}`, where the action is `list`, `view`, `create`,
  `update`, `delete`, `auth`, `authWithPassword`, `requestOTP`, `file`, and so on;
- `METHOD /path`;
- a path prefix ending in `/`.

Each rule can apply only to `@guest` or `@auth` requests, and allows `maxRequests` per
`duration` seconds per client IP. Over the limit, requests get `429`. Superusers and
`excludedIPs` aren't limited. A single instance counts in memory; with several on one
database, the counters are shared through PostgreSQL, so the limits apply across instances.

### Trusted proxy and superuser IPs

Behind a reverse proxy, every request comes from the proxy's address. Set
`trustedProxy.headers` (e.g. `["X-Forwarded-For"]` or `["X-Real-IP"]`) so logs, rate limits,
auth alerts and `superuserIPs` see the client's address. By default the rightmost
`X-Forwarded-For` address is used, the one your proxy added; `useLeftmostIP` takes the first
one instead, which a client can forge unless every proxy in front overwrites the header. Only
set these headers when slonbase can't be reached except through the proxy.

`superuserIPs` (e.g. `["10.0.0.0/8", "203.0.113.7"]`): when set, superuser requests, superuser
sign-in and the `_superusers` endpoints only work from those addresses.

### Batch requests

`POST /api/batch` (`pb.createBatch()` in the SDKs) runs record creates, updates, upserts and
deletes in one transaction: all of them apply, or none, and the response carries the failing
request's error. Enable it with `batch.enabled`. Each request uses the batch's auth and rules and
runs the request hooks.

### Backups

Backups (`/api/backups`, or the dashboard's Backups page) are zip files containing every
collection, the settings, and the local files (unless files are in S3). Restoring one replaces
all of that in one transaction, on every instance. It also restores the installation's signing
key, so tokens issued after the backup stop working.

Backups are kept in `--backups-path`, or in an S3 bucket (`backups.s3`). `backups.cron`
(e.g. `0 3 * * *`, UTC) creates them automatically, and `backups.cronMaxKeep` limits how many are
kept. See [deployment](deployment.md#backups) for how they fit with PostgreSQL's own backups.

**Encryption.** With a backups encryption key (`SLONBASE_BACKUPS_ENCRYPTION_KEY`, at least 12
characters), new backups are encrypted (AES-256-GCM, with a key derived from the passphrase), so
a copy in a bucket or on a laptop reveals nothing without it. They keep their names, download
encrypted, and restore on any installation with the same key; a plain backup uploaded there is
stored encrypted. Restoring needs the key: keep it somewhere other than the backups (a password
manager or your secrets store), because a lost key can't be recovered. The key lives in the
configuration, never in the database, since the database is what's backed up.
`slonbase decrypt-backup <backup> <archive.zip>` turns an encrypted backup back into its zip.

### Auth alerts

`authAlert` (per auth collection, on by default) emails a record when it signs in from an IP
address and user agent it hasn't used before. It isn't sent on the first sign-in, nor on the
first sign-in after a password change. The last 5 devices per record are remembered.

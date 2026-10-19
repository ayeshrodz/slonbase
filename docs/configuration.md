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
| `--database-url` | `SLONBASE_DATABASE_URL` | `database.url` | none (setup mode) |
| `--database-max-connections` | `SLONBASE_DATABASE_MAX_CONNECTIONS` | `database.max_connections` | `16` |
| `--database-ca-file` | `SLONBASE_DATABASE_CA_FILE` | `database.ca_file` | none |
| `--storage-path` | `SLONBASE_STORAGE_PATH` | `storage.path` | `slonbase_data/storage` |
| `--backups-path` | `SLONBASE_BACKUPS_PATH` | `backups.path` | `slonbase_data/backups` |
| `--sql-console` | `SLONBASE_SQL_CONSOLE` | `server.sql_console` | off |
| `--hooks-dir` | `SLONBASE_HOOKS_DIR` | `hooks.dir` | `slonbase_hooks` |
| `--hooks-pool` | `SLONBASE_HOOKS_POOL` | `hooks.pool_size` | `10` |
| `--hooks-watch` | `SLONBASE_HOOKS_WATCH` | `hooks.watch` | `true` |
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

[database]
url = "postgres://slonbase:secret@127.0.0.1:5432/slonbase"
max_connections = 16
# ca_file = "/etc/ssl/certs/rds-global-bundle.pem"

[storage]
path = "slonbase_data/storage"

[backups]
path = "slonbase_data/backups"

[hooks]
dir = "slonbase_hooks"
pool_size = 10
watch = true

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

**SQL console** (`--sql-console`): see [security](deployment.md#security-checklist) before you
turn it on.

## Application settings

These are the sections of `GET/PATCH /api/settings`, and of the dashboard's Settings pages. They
follow PocketBase's names, so its documentation and SDKs apply.

| Section | What it controls |
|---|---|
| `meta` | `appName`, `appURL` (used in email links), `senderName`, `senderAddress`, the dashboard's `accentColor`, `hideControls` |
| `smtp` | Outgoing mail: `enabled`, `host`, `port`, `username`, `password`, `authMethod`, `tls`, `localName`. While it's disabled, emails are written to the log. |
| `s3` | File storage in an S3 bucket instead of `--storage-path` |
| `backups` | `cron` (automatic backups), `cronMaxKeep`, and `s3` (keep backups in a bucket) |
| `logs` | `maxDays` (0 turns request logs off), `minLevel`, `logIP`, `logAuthId` |
| `batch` | `enabled`, `maxRequests`, `timeout`, `maxBodySize` for `/api/batch` |
| `rateLimits` | `enabled`, `rules` |
| `trustedProxy` | `headers` carrying the client IP, `useLeftmostIP` |
| `superuserIPs` | Addresses or ranges superusers may use |

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

### Logs

Every request is logged at level 0, or level 8 with the error for failed requests. Dashboard
files and successful health checks are skipped. slonbase's own warnings and errors are logged
too. Logs are kept for `logs.maxDays` days (5 by default) in an unlogged PostgreSQL table, so
writing them is cheap. They're written in batches in the background, and they don't survive a
PostgreSQL crash. Browse them in the dashboard or with `/api/logs`.

### Rate limits

`rateLimits` (off by default) follows PocketBase's rules. A rule's `label` is one of:

- `{collection}:{action}` or `*:{action}`, where the action is `list`, `view`, `create`,
  `update`, `delete`, `auth`, `authWithPassword`, `requestOTP`, `file`, and so on;
- `METHOD /path`;
- a path prefix ending in `/`.

Each rule can apply only to `@guest` or `@auth` requests, and allows `maxRequests` per
`duration` seconds per client IP. Over the limit, requests get `429`. Superusers and
`excludedIPs` aren't limited. Counters are kept in each instance's memory, so with several
instances the limits apply per instance.

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

### Auth alerts

`authAlert` (per auth collection, on by default) emails a record when it signs in from an IP
address and user agent it hasn't used before. It isn't sent on the first sign-in, nor on the
first sign-in after a password change. The last 5 devices per record are remembered.

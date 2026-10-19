# Running slonbase in production

- [Reverse proxy](#reverse-proxy)
- [systemd](#systemd)
- [Docker](#docker)
- [PostgreSQL](#postgresql)
- [Several instances](#several-instances)
- [Backups](#backups)
- [Monitoring](#monitoring)
- [Security checklist](#security-checklist)

## Reverse proxy

slonbase speaks plain HTTP. Put it behind a reverse proxy that terminates TLS, keep it listening
on `127.0.0.1` (the default), and set the app URL (`meta.appURL`) to the public address, since
email links use it.

Two things matter for the proxy:

- **Realtime** uses Server-Sent Events (`/api/realtime`), which are long-lived responses. The
  proxy must not buffer them, and its read timeout must be longer than slonbase's 30 second
  keep-alive. slonbase sends `X-Accel-Buffering: no`, which nginx honors.
- **Uploads:** raise the proxy's body size limit to your largest file field.

**Caddy** handles both by default:

```
app.example.com {
    reverse_proxy 127.0.0.1:8090
}
```

**nginx:**

```nginx
server {
    listen 443 ssl;
    http2 on;
    server_name app.example.com;
    # ssl_certificate ...; ssl_certificate_key ...;

    client_max_body_size 100m;

    location / {
        proxy_pass http://127.0.0.1:8090;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_buffering off;          # realtime
        proxy_read_timeout 360s;
    }
}
```

Then tell slonbase which header has the client's address: in the dashboard's settings (Trusted proxy),
or `{"trustedProxy": {"headers": ["X-Forwarded-For"]}}`. Without it, logs, rate
limits and `superuserIPs` see the proxy's address. Only set it when slonbase can't be reached
except through the proxy, or clients can send any address they like.

## systemd

The `.deb`/`.rpm` packages install `slonbase.service`. It runs as the `slonbase` user and is
sandboxed: the file system is read-only except `/etc/slonbase` and `/var/lib/slonbase`, there are
no capabilities, and it has a private `/tmp`.

```sh
sudo systemctl status slonbase
sudo journalctl -u slonbase -f
sudoedit /etc/default/slonbase && sudo systemctl restart slonbase   # SLONBASE_BIND, RUST_LOG, ...
```

Any `SLONBASE_*` variable from [configuration](configuration.md) can go in
`/etc/default/slonbase`. To change the flags in `ExecStart`, use a drop-in
(`sudo systemctl edit slonbase`) rather than editing the unit, which upgrades replace.

On a stop or restart, slonbase finishes requests in flight for up to 5 seconds. Realtime clients
are disconnected; the SDKs reconnect and resubscribe on their own.

## Docker

The image runs as an unprivileged user and writes only to `/data`. Its health check
(`slonbase health`) fails while the server starts or can't reach the database. Pin a version tag
(`ghcr.io/ayeshrodz/slonbase:0.2.0`) rather than `latest` in production.

## PostgreSQL

- **Version:** 14 or newer. CI runs on 14 and 17.
- **Role:** slonbase needs a role that can create schemas in its database (`CREATE` on the
  database), such as the database owner. It doesn't need to be a PostgreSQL superuser, and
  shouldn't be one if you enable the SQL console.
- **Connections:** each instance uses up to `--database-max-connections` (16) plus two for
  `LISTEN/NOTIFY`, and up to `projects.max_connections` (4) plus one more for each
  [project](projects.md). Poolers must run in session mode, since transaction mode can't carry
  `LISTEN`.
- **Managed services:** use `sslmode=require`, and `--database-ca-file` when the provider has its
  own CA.
- **Durability vs. write latency:** with PostgreSQL's default `synchronous_commit = on`, every
  write waits for the WAL to reach the disk. `ALTER DATABASE slonbase SET synchronous_commit = off`
  makes writes about 1.5× faster (see the [benchmarks](../bench/README.md)). A crash can then lose
  the last fraction of a second of commits, but never corrupts data. This is the same trade-off
  PocketBase's SQLite makes by default.
- **Your data is ordinary tables** in the `slonbase` schema, so you can query them, replicate
  them, and back them up with the usual tools. Change their structure only through slonbase (the
  dashboard or the collections API), so the collection definitions stay in sync.

## Several instances

Any number of slonbase instances can share one database behind a load balancer. Realtime events,
collection changes, settings and backup restores reach every instance through PostgreSQL
`LISTEN/NOTIFY`. Cron jobs (including automatic backups) run on one instance per scheduled
minute, and migrations run once.

What's per instance:

- **Files:** use S3 storage (`s3` in the settings), or all instances must share the same storage
  directory.
- **Rate limit counters** are kept in each instance's memory.
- **JavaScript hooks:** deploy the same hook files to every instance.

**Realtime needs client affinity.** A realtime client's subscriptions live on the instance that
holds its event stream, and the SDKs set them with a separate `POST /api/realtime`, which must
reach the same instance. Route each client to one instance: by client IP (nginx `ip_hash`,
HAProxy `balance source`) or with a sticky cookie. Everything else can go to any instance.

```nginx
upstream slonbase {
    ip_hash;
    server 10.0.0.11:8090;
    server 10.0.0.12:8090;
}
```

## Backups

There are two complementary ways to back up:

- **slonbase backups** (dashboard → Backups, `/api/backups`, or `backups.cron`) are zip files with
  every collection, the settings and the local files. They're portable between servers and
  PostgreSQL versions, and are restored from the dashboard or with
  `POST /api/backups/{key}/restore`. Keep them off the server with `backups.s3`.
- **PostgreSQL backups** (`pg_dump`, `pg_basebackup`, WAL archiving, or your provider's
  snapshots) cover the whole database, with point-in-time recovery if you need it. Back up the
  storage directory (or bucket) alongside them.

Test a restore before you need one. Restoring a slonbase backup replaces every collection and
the settings on every instance, and signs everyone out, since the signing key is restored too.

## Monitoring

- `GET /api/health` answers `200` when the server is ready (with `"setupRequired": true` while
  it waits for the setup wizard), and `503` while it starts or while the database is
  unreachable. `slonbase health` checks the local server
  and is meant for container health checks.
- Errors and warnings go to stderr (the journal or `docker logs`), and are also stored in the
  request logs (dashboard → Logs, level ≥ 4).
- `GET /api/logs/stats` gives requests per hour.

## Security checklist

- Serve over HTTPS, and keep slonbase on `127.0.0.1` or a private network.
- Set `trustedProxy` when, and only when, every request goes through your proxy.
- Restrict superusers to known addresses with `superuserIPs`, and turn on MFA for the
  `_superusers` collection.
- Leave the **SQL console** off unless you need it. It runs any SQL with the database role's
  privileges, and a PostgreSQL superuser role can read server files or run programs
  (`COPY ... PROGRAM`). When it's on, read-only statements run in read-only transactions, and
  every statement has a 3 minute timeout. Creating or changing view collections also requires
  it, because their queries run with the role's privileges.
- Review API rules: `null` allows superusers only, and `""` allows anyone.
- Turn on rate limits for sign-in and other public endpoints.
- Keep the config file private (`0600`, which the wizard sets), since it contains the database
  password.
- Report vulnerabilities as described in [SECURITY.md](../SECURITY.md).

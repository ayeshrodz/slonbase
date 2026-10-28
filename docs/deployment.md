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

The image runs as an unprivileged user (uid 65532) and writes only to `/data`. Docker volumes
keep that ownership. A disk or bind mount owned by root doesn't: slonbase starts, but file
uploads fail with "Permission denied". `chown 65532:65532` the directory, or run the container as
root, as the [platform setups](platforms.md) do. Its health check
(`slonbase health`) fails while the server starts or can't reach the database. Pin a version tag
(`ghcr.io/ayeshrodz/slonbase:1.0.0`) rather than `latest` in production.

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
  the last fraction of a second of commits, but never corrupts data. (Embedded SQLite backends
  make the same trade-off by default.)
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
- **JavaScript hooks:** deploy the same hook files to every instance.

**No client affinity needed.** A realtime client's event stream lives on one instance, and the
SDKs set its subscriptions with a separate `POST /api/realtime`. When that request reaches
another instance, it's forwarded through PostgreSQL to the one holding the stream, which checks
the request's auth and answers. So a plain round-robin load balancer works. (Affinity, like
nginx `ip_hash`, still saves that hop.)

**Rate limits are shared.** While an instance is alone, it counts requests in memory. When
others share the database, the counters move to an unlogged PostgreSQL table, so a client gets
the configured limit across all instances, not that limit on each. Instances find each other
with a heartbeat every 20 seconds in `_slonbase.instances`.

## Backups

There are two complementary ways to back up:

- **slonbase backups** (dashboard → Backups, `/api/backups`, or `backups.cron`) are zip files with
  every collection, the settings and the local files. They're portable between servers and
  PostgreSQL versions, and are restored from the dashboard or with
  `POST /api/backups/{key}/restore`. Keep them off the server with `backups.s3`, and encrypt
  them with a [backups encryption key](configuration.md#backups) when they leave it.
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
- **Prometheus metrics** at `/metrics` on a separate address, with `--metrics-bind
  127.0.0.1:9090` (or `SLONBASE_METRICS_BIND`). The endpoint isn't authenticated, so bind it to a
  private interface. It has:
  - `slonbase_http_requests_total{project,method,route,status}` and
    `slonbase_http_request_duration_seconds` (a histogram), by route pattern
    (`/api/collections/{collection}/records`), so the series stay few;
  - `slonbase_db_connections{state="idle|in_use|max"}` and `slonbase_db_waiting` (requests
    waiting for a pooled connection: a sign the pool is too small);
  - `slonbase_realtime_clients`, `slonbase_realtime_events_total`, `slonbase_instances`;
  - `slonbase_info{version}` and `process_start_time_seconds`.

  ```yaml
  # prometheus.yml
  scrape_configs:
    - job_name: slonbase
      static_configs:
        - targets: ["10.0.0.11:9090", "10.0.0.12:9090"]
  ```

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
- Keep rate limits on (new installations start with them on), and turn them on for
  installations from before 0.4.2.
- Keep the config file private (`0600`, which the wizard sets), since it contains the database
  password.
- Report vulnerabilities as described in [SECURITY.md](../SECURITY.md).

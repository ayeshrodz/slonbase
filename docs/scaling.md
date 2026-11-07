# Scaling and deployment topologies

slonbase runs as one process or as several, on one PostgreSQL server or on more than one. This
guide describes the five layouts that make sense, what each one is for, what was tested, and the
limits you will hit. Everything here was run, not only reasoned about: the scripts are in
[`scripts/fleet-test`](https://github.com/ayeshrodz/slonbase/tree/main/scripts/fleet-test) and run
in CI on every change (three instances behind a balancer with no affinity, directly and through
PgBouncer in transaction mode).

- [Which layout](#which-layout)
- [A. One instance](#a-one-instance)
- [B. One instance, several projects, several servers](#b-one-instance-several-projects-several-servers)
- [C. Several instances behind a load balancer](#c-several-instances-behind-a-load-balancer)
- [D. Several instances behind PgBouncer](#d-several-instances-behind-pgbouncer)
- [E. Managed PostgreSQL with failover](#e-managed-postgresql-with-failover)
- [The connection budget](#the-connection-budget)
- [What is shared and what is per instance](#what-is-shared-and-what-is-per-instance)
- [Sign-in, tokens and sign-out across instances](#sign-in-tokens-and-sign-out-across-instances)
- [Upgrades](#upgrades)
- [Balancer recipes](#balancer-recipes)
- [Limits, and how to find yours](#limits-and-how-to-find-yours)

## Which layout

| You want | Use |
|---|---|
| The simplest thing that works, up to thousands of requests a second | [A](#a-one-instance) |
| Several isolated apps, or one app's data on another server | [B](#b-one-instance-several-projects-several-servers) |
| Survive an instance or machine failing; more cores than one machine has | [C](#c-several-instances-behind-a-load-balancer) |
| C with many instances or many projects, where connections run out | [D](#d-several-instances-behind-pgbouncer) |
| A managed database with a standby that takes over | [E](#e-managed-postgresql-with-failover) |

Start with A. One instance on a modest machine serves far more than most apps need (see the
[benchmarks](benchmarks.md)), and a second instance is an operations decision, not a performance
one, until a single machine's CPUs are the limit.

## A. One instance

![One slonbase instance on one PostgreSQL server](assets/topology-single.svg)

One `slonbase serve`, one PostgreSQL database. Nothing to coordinate. Idle it holds about 3
connections to PostgreSQL, and up to 18 under load (a pool of 16, `database.max_connections`,
plus two that wait for notifications).

## B. One instance, several projects, several servers

![One slonbase serving a main database and projects on two PostgreSQL servers](assets/topology-projects.svg)

[Projects](projects.md) are isolated installations (their own users, collections, settings and
database) served by one slonbase. By default each project's database is created next to the main
one, on the same PostgreSQL server. A project can instead **connect an existing database** on any
other PostgreSQL 14+ server with `databaseUrl`, so one slonbase can serve a main database on
server A and projects on servers A, B and C. Several instances can do the same: every instance of
the main installation serves every project, and must be able to reach every server.

Tested: a project on a second server serves requests alongside the main installation. When that
server was stopped, the main installation and its other projects kept answering; the remote
project returned errors, and recovered about three seconds after the server came back.

Each project pool is its own set of connections (up to `projects.max_connections`, 4, plus one for
notifications) **on the project's server**, so the [connection budget](#the-connection-budget) is
per server.

## C. Several instances behind a load balancer

![Several slonbase instances behind a load balancer on one PostgreSQL server](assets/topology-fleet.svg)

Any number of instances can share one database behind any load balancer, with **no sticky
sessions**. That is what was tested: round-robin with no affinity, three instances (and six for
the connection numbers), checking that:

- a token issued by one instance is accepted by every other (30 requests out of 30 spread over 3);
- a new collection, a new field and a settings change reach the other instances in milliseconds
  (3 to 13 ms measured on one machine);
- a rate limit of 30 allows exactly 30 requests across three instances;
- realtime events written through any instance, or by plain SQL, reach a stream held by one
  instance (4 to 27 ms), and a subscribe request that lands on another instance is forwarded to
  the one holding the stream;
- automatic backups and cron jobs run once per scheduled minute, not once per instance;
- a project created through one instance serves on all of them within about half a second.

Two things need your attention, because they are **per instance** unless you make them shared:

1. **Files.** The default storage is a local directory, so a file uploaded through one instance is
   a 404 on the others. Use S3 storage, or mount one directory on every instance. Health shows a
   warning when instances share a database but the storage is local.
2. **Connections.** Each instance adds to the total. Plan the [connection budget](#the-connection-budget).

If an instance is killed, its realtime clients reconnect through the balancer (the SDKs do this
on their own). The other instances notice it is gone within a minute, and requests that were
waiting on it for a subscribe answer give up after two seconds rather than five.

## D. Several instances behind PgBouncer

![Several slonbase instances behind PgBouncer, with listeners connected directly](assets/topology-pooler.svg)

A pooler lets many instances and projects share a small number of PostgreSQL connections. slonbase
works behind **PgBouncer in transaction mode** (1.21 or newer) when you do two things:

- set `max_prepared_statements` (for example 300) in PgBouncer, so slonbase's prepared statements
  work across transactions, and `ignore_startup_parameters = extra_float_digits,options`;
- give slonbase a **direct** connection for its listeners: `--database-listen-url` (or
  `SLONBASE_DATABASE_LISTEN_URL`, or `database.listen_url`) pointing at the same server without
  the pooler. `LISTEN` needs a session, which a transaction pooler cannot give.

```sh
SLONBASE_DATABASE_URL='postgres://app:secret@pgbouncer:6432/slonbase' \
SLONBASE_DATABASE_LISTEN_URL='postgres://app:secret@db:5432/slonbase' \
  slonbase serve
```

Without `listen_url`, behind a transaction-mode pooler, slonbase starts and answers requests, but
the instances stop hearing each other: schema changes, settings, realtime events and rate limits
are no longer shared. slonbase detects this: each instance sends itself a notification every
minute, and Health shows a **sync** error that names the likely cause and the fix.

A project's own database is never routed through the main installation's pooler: a project
connected with its own `databaseUrl` uses that URL, and a project on the main server uses the main
`database.url` host with its own database name (so it goes through the pooler, if the pooler
serves that database).

Measured, six instances and four projects behind PgBouncer (`default_pool_size = 10`): 83
connections to PostgreSQL in total, with no connection errors, where the same layout without a
pooler hit 97 of 100 connections and returned HTTP 500 errors. Direct listeners still cost
connections: each instance keeps `2 + projects` of them, so the number of instances and projects
still matters (see below).

## E. Managed PostgreSQL with failover

Managed services (RDS, Cloud SQL, Neon, Supabase, Crunchy and others) fail over by moving a DNS
name or a proxy to a standby. slonbase's pools drop broken connections and open new ones;
while the database is unreachable, requests fail with an error response rather than hanging, and
the listeners reconnect with backoff, after which the instances catch up on what they missed from
the event outbox. Use the provider's **writer endpoint** (the one that follows
the primary), `sslmode=require` and `--database-ca-file` if the provider has its own CA. A pooler
offered by the provider (RDS Proxy, Supabase's pooler, Neon's) is a transaction pooler: use
[D](#d-several-instances-behind-pgbouncer), with the provider's **direct** endpoint as
`listen_url`.

This was verified here only by stopping and starting a PostgreSQL server (recovery in about three
seconds), not against a real managed failover. Treat the failover time of your provider as part of
your own downtime budget, and test it before you depend on it.

## The connection budget

PostgreSQL's `max_connections` (100 by default) is the limit that matters first. slonbase's
use of it is predictable:

| | Connections per instance |
|---|---|
| The main database | up to `database.max_connections` (16), plus 2 for notifications |
| Each project | up to `projects.max_connections` (4), plus 1 for notifications |

So **N instances and P projects use at most N × (18 + 5 × P)**, with connections opened on demand
and idle project connections released after two minutes. Measured on one PostgreSQL 16 server:

| Layout | Idle | Under load |
|---|---|---|
| 1 instance | 3 | 18 |
| 3 instances | 11 | 54 |
| 3 instances, 4 projects | 41 | 97 of 100 |
| 6 instances, 4 projects | | 97 of 100, with HTTP 500 "too many clients" |
| 6 instances, 4 projects, PgBouncer | | 83, no errors |

The levers, in the order to reach for them:

1. **Lower the pools.** `database.max_connections` of 16 is generous: a pool of 8 serves most
   workloads, since each query is short. `projects.max_connections` of 2 is enough for most
   projects.
2. **Raise PostgreSQL's `max_connections`**, and mind the memory (each connection costs a few MB
   plus its `work_mem`). A few hundred is normal on a server with enough RAM.
3. **Use a pooler** ([D](#d-several-instances-behind-pgbouncer)).

slonbase warns before you run out: Health shows the connections in use against the server's
limit, a warning from 80% and an error from 95%.

## What is shared and what is per instance

Shared, through PostgreSQL, with nothing to configure: records and files' metadata, collection
and field changes, settings, the signing secrets and token keys, rate limit counters, realtime
events and subscriptions, cron job runs and automatic backups (once per scheduled minute),
migrations (once, behind an advisory lock) and the list of live instances.

Per instance, and yours to keep identical:

- **File storage** unless it is S3 or a shared volume;
- **JavaScript hooks and migration files** (`pb_hooks`, `pb_migrations`): deploy the same files
  to every instance;
- **The settings encryption key**, if you use one: the same on every instance;
- **The configuration file and flags**: pool sizes, bind address, trusted proxy headers.

## Sign-in, tokens and sign-out across instances

There is nothing to stick to. A sign-in produces a signed token (a JWT) whose key is stored in the
database, and any instance accepts it: tokens are not held in an instance's memory. In the test,
a superuser and a user token were accepted by all three instances, request after request.

There is also no server-side session list, so there is no "logout" call, exactly as in PocketBase:
a client signs out by discarding its token. What ends a token everywhere at once, on every
instance, immediately:

- **changing the user's password**, or deleting the user;
- **`POST /api/collections/{collection}/records/{id}/revoke-tokens`**, "sign out everywhere": it
  gives the record a new token key. The user may call it for themselves; a superuser may call it
  for anyone (the **Sign out everywhere** action in the dashboard's record menu). Old tokens
  answered 401 on all three instances at once in the test.

Short token lifetimes (the token durations in the collection's auth options) limit what a stolen token
can do in the meantime, as with any stateless token.

## Upgrades

Stop all instances, start the new version on one (it runs the migrations once), then start the
rest. **Rolling upgrades are not supported:** old and new instances must not serve at the same
time across a release with a migration, and not every release says it has one. With a balancer in
front, take the site into maintenance mode, or briefly accept errors, for the length of a
restart (seconds).

## Balancer recipes

Any balancer works. Two things matter: **do not buffer the realtime stream**, and use
**`/api/health`** as the health check (it answers `200` when the instance and its database
connection are fine, and is cheap enough to poll every second).

### nginx (tested)

```nginx
upstream slonbase {
    least_conn;
    server 10.0.0.11:8090 max_fails=2 fail_timeout=5s;
    server 10.0.0.12:8090 max_fails=2 fail_timeout=5s;
    server 10.0.0.13:8090 max_fails=2 fail_timeout=5s;
    keepalive 32;
}
server {
    listen 443 ssl;
    server_name api.example.com;
    client_max_body_size 100m;
    location / {
        proxy_pass http://slonbase;
        proxy_http_version 1.1;
        proxy_set_header Connection "";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_buffering off;      # realtime events must not wait in a buffer
        proxy_read_timeout 1h;    # the event stream stays open
    }
}
```

Tested with three instances behind nginx 1.24: a realtime stream opened through nginx received an
event written through a different instance at once, a subscribe request routed to another
instance was forwarded, and with one instance killed every request still succeeded. Set the
trusted proxy headers in slonbase's settings so rate limits and logs see the client's address.

### HAProxy (not run here)

```
backend slonbase
    balance leastconn
    option httpchk GET /api/health
    timeout server 1h
    server a 10.0.0.11:8090 check inter 2s
    server b 10.0.0.12:8090 check inter 2s
    server c 10.0.0.13:8090 check inter 2s
```

### PgBouncer (tested)

```ini
[databases]
slonbase = host=127.0.0.1 port=5432

[pgbouncer]
listen_port = 6432
auth_type = scram-sha-256
auth_file = /etc/pgbouncer/userlist.txt
pool_mode = transaction
max_client_conn = 3000
default_pool_size = 10
max_prepared_statements = 300
ignore_startup_parameters = extra_float_digits,options
```

`ignore_startup_parameters` includes `options` because slonbase sends one startup option
(`slonbase.notify=off`, used to skip a per-write notification). PgBouncer drops it, which means
writes behind a pooler also issue that per-write notification. Realtime events are delivered from
the event outbox either way; the cost of the extra notification per write behind a pooler was not
measured.

### systemd, several instances on one machine (not run here)

Copy the packaged `slonbase.service` to `slonbase@.service`, change `--bind` to
`--bind 127.0.0.1:%i` and give each instance its own state directory, then
`systemctl enable --now slonbase@8091 slonbase@8092`. Instances on several machines need no
template: install the package and the same configuration on each.

### Docker Compose (not run here)

```yaml
services:
  db:
    image: postgres:17
    environment: { POSTGRES_PASSWORD: secret, POSTGRES_DB: slonbase }
    volumes: [pgdata:/var/lib/postgresql/data]
  slonbase:
    image: ghcr.io/ayeshrodz/slonbase:latest
    environment:
      SLONBASE_DATABASE_URL: postgres://postgres:secret@db:5432/slonbase?sslmode=disable
    deploy: { replicas: 3 }
    depends_on: [db]
  nginx:
    image: nginx:1.27
    ports: ["8090:80"]
    depends_on: [slonbase]
volumes:
  pgdata:
```

Compose resolves `slonbase` to every replica, but nginx resolves a name once at start: use
nginx's `resolver 127.0.0.11` with a variable in `proxy_pass`, or list the replicas explicitly.
Files then need S3 or a shared volume, as in [C](#c-several-instances-behind-a-load-balancer).

## Limits, and how to find yours

Tested here: up to six instances, up to four projects, one and two PostgreSQL servers, PgBouncer
1.22 in transaction mode, PostgreSQL 16 on one machine. Beyond that the structure is the same,
but the numbers are yours to measure:

```sh
FLEET_ADMIN_URL='postgres://postgres:...@host:5432/postgres' scripts/fleet-test/run.sh 6
```

Known limits:

- **Connections** grow with instances × projects (see above); the listeners cannot go through a
  transaction pooler, so even behind PgBouncer each instance holds `2 + projects` direct
  connections.
- **Realtime fan-out** is one PostgreSQL `LISTEN` per instance, and every instance reads the
  outbox: very write-heavy installations with many instances add read load to PostgreSQL.
- **Rolling upgrades** are not supported ([Upgrades](#upgrades)).
- **Local files** are per instance ([C](#c-several-instances-behind-a-load-balancer)).
- **One PostgreSQL primary** takes all the writes. slonbase does not shard, and does not send
  reads to replicas; replicas are for your own reporting, backups and failover.

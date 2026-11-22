# Projects

One slonbase installation can host several **projects**. Each project is a separate
installation, with its own:

- collections and records;
- superusers and users;
- settings, files, backups, logs and hooks.

All of them run in the same slonbase process, so you don't run (and update) one server per app.
Their databases are created on the main installation's PostgreSQL server, or [connected](#databases-on-other-servers)
from any other PostgreSQL server. The main installation's **command center** shows and controls
them all.

> [!NOTE]
> Projects are new in 0.2.

## How it works

- **Databases.** Each project lives in a PostgreSQL database of its own, on the server the main
  installation uses. The database is named `<main database>_<project>`, for example
  `slonbase_blog`. Inside it are the usual `slonbase` and `_slonbase` schemas, so a project can
  be dumped, restored or moved like any slonbase database.
- **URLs.** A project is served under `/p/<name>/`. Its API is at `/p/blog/api/...` and its
  dashboard at `/p/blog/_/`. Point the SDK at the project's URL:

  ```js
  const pb = new PocketBase("https://example.com/p/blog");
  ```

  A project can also have [host names](#host-names) of its own.

- **Isolation.** Superusers and tokens are separate. Signing in to one project gives no access
  to another, nor to the main installation. Settings are separate too, including SMTP, S3, rate
  limits and superuser IPs.
- **Files and hooks.** Each project keeps them under `projects.path`
  (`slonbase_data/projects/<name>/`; `/var/lib/slonbase/projects/` for the packages and
  `/data/projects/` in Docker):

  | Path | |
  |---|---|
  | `<name>/storage/` | uploaded files (unless the project's settings use S3) |
  | `<name>/backups/` | backups |
  | `<name>/hooks/` | JavaScript hooks (`*.pb.js`) |
  | `<name>/migrations/` | [migrations](migrations.md) |

  When a project's hook files change, only that project restarts (unless `hooks.watch` is
  off). Requests already in progress finish on the old version, and realtime clients
  reconnect.
  Every project that has hook files starts its own JavaScript runtimes, one thread each:
  `hooks.pool_size` of them (10 by default), and more on demand while requests wait (those
  stop again, gradually, after 30 seconds without work). With many
  projects that have hooks, the threads add up (projects × `pool_size`), so lower
  `hooks.pool_size` (`--hooks-pool`) for a fleet: a runtime runs one hook at a time, so a
  project with light hooks needs few.
- **Connections.** Each project opens up to `projects.max_connections` (4) pooled connections,
  plus one for realtime. Keep the total within PostgreSQL's `max_connections`.
- **Bulk imports.** A batch (`POST /api/batch`) or an upload holds a connection while it runs.
  slonbase lets those take turns, all but one connection at a time, so the dashboard and the
  API's other requests always have one free; extra batches wait their turn. Importing with
  more workers than the pool has connections (4 by default) therefore doesn't speed it up: use
  about as many workers as there are connections less one, or raise `projects.max_connections`.
  Batches of 1,000 to 2,000 records work well. Request logging is often switched off for an
  import (`logs.maxDays` of 0); switch it back on afterwards, or traffic isn't counted.
- **Several instances.** Every slonbase instance on the main database serves every project.
  Projects created or deleted on one instance start or stop on the others within moments.

Only **superusers of the main installation** manage projects. Rust extensions apply to the main
installation only.

## What a project costs

Measured with the scripts in [`bench/`](../bench/) (`footprint.mjs`; a release build of 2.7, PostgreSQL 16 on the same
4-vCPU machine, Linux, an idle process first, then traffic to every project):

| Projects | Memory (RSS) without hooks | Memory with hooks (`hooks.pool_size` 10) | Threads with hooks (10) | Memory with hooks (`hooks.pool_size` 2, idle) | Database connections |
|--:|--:|--:|--:|--:|--:|
| 0 (the main installation) | 37 MB | 36 MB | 6 | 36 MB | 5 |
| 1 | 41 MB | 82 MB | 16 | 51 MB | 8 to 10 |
| 5 | 51 MB | 228 MB | 57 | 87 MB | 23 to 30 |
| 20 | 50 MB | 729 MB | 207 | 243 MB | 76 to 105 |
| 50 | 61 MB | 1.8 GB | 512 | 836 MB | 206 to 255 |

- **A project without hooks is cheap:** about 0.5 MB and no threads of its own, and four database
  connections (`projects.max_connections`), plus one for realtime while it is serving.
- **A project with hooks costs roughly 11 MB plus about 2.4 MB for each runtime** of its pool: about
  35 MB and 10 threads at the default `hooks.pool_size`, about 16 MB and 5 threads with 2. Lower
  `hooks.pool_size` (`--hooks-pool`) for a fleet of projects whose hooks are light.
- **Runtimes are added when requests arrive together.** A project that receives concurrent requests
  starts more runtimes than `hooks.pool_size`, and they stop again after 30 seconds without work, one
  after another, so a burst on every project takes the memory of the default pool or more for a few
  minutes (with 50 projects and 30 concurrent requests each: about 2.3 GB, 690 threads). Plan for
  the bursts, not the idle numbers.
- **Database connections** are 4 or 5 for each project (up to 5 once it serves requests), and
  about 5 for the main installation. Keep `max_connections` of PostgreSQL above 5 per project plus
  the main installation's, plus the connections of the other instances and of anything else that uses
  the server (the table's 50 projects needed 255 at most, so 600 was set for the runs).
- Creating a project takes about 0.5 seconds, and 1.4 seconds with hooks (a project restarts when its
  hook files are written).

Hook workers are not shared between projects: each project keeps its own runtimes, so a project's
hooks never run in another project's runtime. The numbers above are the cost of that isolation.

Deleting a record that 10,000 records refer to (relations with cascade delete) takes about 2.3
seconds, and 7.8 seconds when the referring collection has a delete hook (the hook runs for each
record); a batch of 100 creates takes 18 ms, against 176 ms for 100 single requests.

## Host names

Besides `/p/<name>/`, a project can be served on host names of its own, such as
`shop.example.com`. Requests whose `Host` header matches go to the project at the root path:
its API at `https://shop.example.com/api/` and its dashboard at `/_/`.

1. Point the name's DNS at the server.
2. Have your reverse proxy accept it, with a certificate, and pass the `Host` header on. Caddy
   and the [nginx example](deployment.md#reverse-proxy) do.
3. Add the name in the dashboard (the project's menu → Host names), with
   `slonbase project hosts shop shop.example.com`, or with
   `PATCH /api/projects/shop {"hosts": ["shop.example.com"]}`.
4. Set the project's app URL in its settings to the new address, so emails link to it.

A host name belongs to one project at most. Ports and letter case are ignored.

## An own database role

By default, a project's database belongs to the main installation's role. With **own role**
(a switch when creating a project, `--own-role` on the command line, or `"ownRole": true` in the
API), the project gets a PostgreSQL role of its own, named like its database. That role:
- owns the project's database and tables;
- is the role the project connects with;
- is, apart from the main installation's role, the only one that can connect: `PUBLIC` loses
  `CONNECT` on the database.

So a project's hooks and SQL console can't reach other projects' data. The role is created with
a random password, kept in the main database, and dropped with the project. This needs
`CREATEROLE` as well as `CREATEDB`:

```sql
ALTER ROLE slonbase CREATEDB CREATEROLE;
```

## Databases on other servers

Instead of creating a database on the main installation's server, a project can **connect an
existing database**, on any PostgreSQL 14+ server, by URL:

```sh
SLONBASE_PROJECT_DATABASE_URL='postgres://shop:secret@db2.internal:5432/shop?sslmode=require' \
  slonbase project create shop --email owner@example.com --connect "$SLONBASE_PROJECT_DATABASE_URL"
```

or **Connect a database** in the dashboard's new-project dialog, or `"databaseUrl"` in the API.

- slonbase connects, adds its tables to the database (as on any new installation) and creates the
  first superuser. A database that **already holds a slonbase installation** keeps its data,
  superusers and settings: this is how you bring an installation in, or move a project to
  another server (restore a backup there, then connect it).
- The URL's role is the project's role (no **own role**), and needs the same rights as the main
  installation's role on its database. The URL's TLS settings (`sslmode`) and the main
  installation's `--database-ca-file` apply.
- slonbase refuses the main installation's own database, a database another project already uses,
  and a server it can't reach.
- The URL, with its password, is stored in the main database, **encrypted** when the settings
  [encryption key](configuration.md#encrypting-the-settings) is set (and re-encrypted by
  `slonbase rekey`). It's never sent back by the API; the dashboard shows the server and
  database only.
- **Deleting** the project stops it and removes it from the list, but leaves the database as it
  is, since slonbase didn't create it.
- Every instance of the main installation must be able to reach the other server. Each project
  holds its own connections on its own server (up to `projects.max_connections` plus one
  listener per instance), which the [scaling guide](scaling.md#the-connection-budget) counts.

## Requirements

To create databases on the main installation's server, its database role must be allowed to:

```sql
ALTER ROLE slonbase CREATEDB;
```

Without it, creating a project fails with an explanation.

## The command center

The main installation's **Projects** page is a command center for every installation it hosts,
the main one included, updated every 5 seconds (it pauses while the tab is hidden, or with
**Live**):

- **Fleet totals:** installations running, requests in the last 24 hours and per minute now,
  server errors, the slowest p95 latency, data size and records, realtime clients.
- **Requests per minute** over the last hour, one line per installation (click a name to hide
  it), and how traffic, storage and records divide between them.
- **A card per installation** (or a sortable table): status, server and database, a sparkline
  of the last hour, requests, errors, p95, records, connections in use, the database's size and
  round trip, and its configuration at a glance (mail, files, backups, rate limits,
  encryption). Filter by name, by those that need attention, or by those on other servers.
- **A drawer per installation:** its charts (per minute for the last hour, per hour for the
  last day) and numbers; its **settings**, which a superuser of the main installation can change
  (application name and URL, sender, SMTP, backup schedule and how many to keep, log retention,
  rate limits, locking collection editing); its connection, API and SDK URLs; and **restart**,
  **host names** and **delete**.

Settings changed here are validated and saved as the project's own `PATCH /api/settings` would
save them, and every instance picks them up; the project's `onSettingsUpdateRequest` hooks
don't run, since the request isn't the project's. Everything else (OAuth2 providers, S3,
collections, the project's superusers) is in the project's own dashboard. Projects have no
command center of their own: only the main installation sees and controls them all.

From the Projects page you also:
- create a project, with its first superuser, on this server or by connecting a database;
- delete a project. You type its name to confirm, and a backup is made first unless you switch
  it off.

The **switcher** next to the logo shows which installation a dashboard manages. When the
browser is also signed in to the main installation, it lists the projects too, so you can go
from one to another. Each dashboard keeps its own session, because each project has its own
superusers.

## Command line

```sh
slonbase project list
SLONBASE_SUPERUSER_PASSWORD='...' slonbase project create blog --email owner@example.com
slonbase project delete blog                # asks for the name again; --confirm blog skips the question
slonbase project delete blog --no-backup    # without the backup first
slonbase project create shop --email owner@example.com --own-role
slonbase project create crm --email owner@example.com --connect 'postgres://crm:secret@db2:5432/crm'   # or SLONBASE_PROJECT_DATABASE_URL
slonbase project hosts shop shop.example.com www.shop.example.com   # --clear removes them
slonbase project hosts shop                 # lists them
```

Use the same `--config` (or environment) as the server; for the packages that's
`sudo -u slonbase slonbase --config /etc/slonbase/config.toml project ...`. Running servers start
and stop projects created or deleted from the command line within moments.

`--project <name>` runs the other offline commands on a project instead of the main
installation:

```sh
slonbase superuser upsert owner@example.com 'new password' --project blog
slonbase import-pocketbase /path/to/pb_data --project blog    # move a PocketBase app into a project
```

## API

All endpoints need a superuser of the main installation.

| Endpoint | |
|---|---|
| `GET /api/projects` | The projects: `name`, `database`, `path`, `status`, `created`, `external`, `server` |
| `GET /api/projects/overview` | Every installation (the main one first) with its status, database, pool, records, traffic (`requests`, `errors`, `avgMs`, `p95Ms`, `perMinute` for the last 60 minutes, `perHour` for the last 24 hours) and a configuration summary |
| `GET /api/projects/{name}` | One project |
| `POST /api/projects` | Create a project: `{"name", "superuserEmail", "superuserPassword", "ownRole"}`, or connect a database with `"databaseUrl"` |
| `PATCH /api/projects/{name}` | Set its host names: `{"hosts": [...]}` |
| `GET /api/projects/{name}/settings` | A project's settings (passwords and secrets left out) |
| `PATCH /api/projects/{name}/settings` | Change a project's settings, as its own `PATCH /api/settings` |
| `POST /api/projects/{name}/restart` | Restart a project (its hooks are loaded again, its connections opened afresh) |
| `DELETE /api/projects/{name}?confirm={name}` | Delete a project (`&backup=false` skips the backup); answers `{"backup", "files", "databaseKept"}` |

```sh
curl -X POST localhost:8090/api/projects -H "Authorization: $TOKEN" -H 'content-type: application/json' \
  -d '{"name": "blog", "superuserEmail": "owner@example.com", "superuserPassword": "a-long-password"}'
```

Names are 2 to 32 lowercase letters, digits and dashes, starting with a letter.

Creating a project:
1. creates its database and slonbase's system tables in it;
2. creates the project's first superuser;
3. sets the project's app name, and its app URL to the main app URL plus `/p/<name>`.

If any step fails, the database is dropped again.

A project's `status` is one of:
- `ready`;
- `starting`, with the reason in `statusDetail` (for example, its database is unreachable or its
  hooks failed to load);
- `setup`.

**Deleting a project drops its database**, so the name must be repeated (`confirm`). Before
that, slonbase backs the project up (`pre-delete-<time>.zip`). If the backup fails, nothing is
deleted. The project's files and backups are kept, renamed to `<name>.deleted-<time>` in the
projects directory, so the backup survives the delete. If the project's backups go to S3, the
backup is kept there. To restore a deleted project:
1. create a new project;
2. upload the backup to it (`/p/<name>/api/backups/upload`, or its dashboard's Backups page);
3. restore it.

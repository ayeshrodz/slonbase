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

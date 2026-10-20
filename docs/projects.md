# Projects

One slonbase installation can host several **projects**. Each project is a separate
installation, with its own:

- collections and records;
- superusers and users;
- settings, files, backups, logs and hooks.

All of them run on the same PostgreSQL server and in the same slonbase process. PocketBase has no
equivalent: there you run one process per app.

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

## Requirements

The database role in the main installation's connection must be allowed to create databases:

```sql
ALTER ROLE slonbase CREATEDB;
```

Without it, creating a project fails with an explanation.

## Dashboard

The main installation's dashboard has a **Projects** page, where you can:
- create a project, with its first superuser;
- see each project's status, database and SDK URL;
- open a project's dashboard;
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
| `GET /api/projects` | The projects: `name`, `database`, `path`, `status`, `created` |
| `GET /api/projects/{name}` | One project |
| `POST /api/projects` | Create a project: `{"name", "superuserEmail", "superuserPassword", "ownRole"}` |
| `PATCH /api/projects/{name}` | Set its host names: `{"hosts": [...]}` |
| `DELETE /api/projects/{name}?confirm={name}` | Delete a project (`&backup=false` skips the backup); answers `{"backup", "files"}` |

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

# Changelog

All notable changes to slonbase are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). Before 1.0, a minor version (0.x) may contain breaking
changes, and they'll be listed here.

## [Unreleased]

### Added

- **JavaScript migrations**, compatible with PocketBase's `pb_migrations`: `*.js` files in the
  migrations directory (`--migrations-dir`, `slonbase_migrations` by default) that call
  `migrate(up, down)`. See [docs/migrations.md](docs/migrations.md).
  - The server applies pending migrations when it starts. Each runs in a transaction of its
    own, so a failing migration changes nothing; the server then waits, with the error as its
    status, and tries again.
  - Several instances apply each migration once. The applied list is kept in the database and
    in backups.
  - `slonbase migrate up|down|create|collections|history-sync|list`, with `--project`.
  - Automigrate (`--automigrate`, off by default) writes a migration for each collection
    change made in the dashboard or with the collections API.
  - PocketBase's own migration files, automigrate output and snapshots run as they are, and
    `import-pocketbase` marks PocketBase's applied migrations as applied.
- In hooks and migrations, `$app.save()` and `$app.delete()` take collections (and `save` the
  settings) as well as records, with `new Collection({...})`, `new Field({...})` (and
  `TextField`, `NumberField`, ...), `collection.fields.add/addAt/removeById/...`,
  `unmarshal()`, `$app.importCollections()` and `$app.findAllCollections()`.

### Fixed

- Importing collections (the dashboard's import or the API) creates view collections whose
  queries read tables created by the same import, and accepts PocketBase's exports with its
  system collections.

## [0.2.1] - 2026-10-20

Package repositories, so `apt upgrade` and `dnf upgrade` bring new versions, and a new look
for the dashboard. Nothing changes in the database or the API.

### Added

- Signed apt and dnf repositories at https://ayeshrodz.github.io/slonbase/, updated with every
  release, so `apt upgrade` and `dnf upgrade` install new versions. They have every release since 0.1.0.
  See [installation](https://github.com/ayeshrodz/slonbase/blob/v0.2.1/docs/installation.md#debian-ubuntu-fedora-rhel-packages).

### Changed

- The dashboard has a new palette: cool neutrals, a muted indigo accent for buttons, tabs and
  charts, and muted colors for collections, field types, select values and status. Light and
  dark themes both. A custom dashboard accent from the settings still applies.

## [0.2.0] - 2026-10-19

**Projects:** one slonbase installation can now host several isolated projects. Each project is
a complete installation of its own, with its own collections, superusers, settings, files,
backups, logs and hooks, in a PostgreSQL database of its own on the same server. PocketBase has
no equivalent. See [docs/projects.md](https://github.com/ayeshrodz/slonbase/blob/v0.2.0/docs/projects.md).

### Added

- **Projects**, served under `/p/<name>/`, API and dashboard included. Point the SDK at
  `https://example.com/p/<name>`.
  - Superusers of the main installation create and delete them from the dashboard's Projects
    page, `slonbase project create|list|delete`, or `/api/projects`.
  - Creating a project makes its database, slonbase's system tables in it and its first
    superuser. If a step fails, everything is removed again.
  - Deleting a project asks for its name again and backs it up first. The backup is kept with
    the project's files.
  - Every instance on the main database serves every project, and changes reach all of them
    within moments.
- **Host names** for projects (`shop.example.com`), set from the dashboard,
  `slonbase project hosts` or `PATCH /api/projects/{name}`. Requests are routed by their
  `Host` header.
- **An own database role** for a project, as an option: a PostgreSQL role that owns the
  project's database, which the project connects with and nobody else can.
- Changing a project's hook files restarts only that project.
- A project switcher in the dashboard's top bar, and projects in the command palette.
- `--project <name>` runs `superuser` and `import-pocketbase` on a project, for example to
  move a PocketBase app into one.
- The `projects.path` (`--projects-path`, `SLONBASE_PROJECTS_PATH`) and
  `projects.max_connections` settings.

### Upgrading from 0.1

Install the new version as usual (see
[upgrading](https://github.com/ayeshrodz/slonbase/blob/v0.2.0/docs/installation.md#upgrading)).

- Two system migrations (7 and 8) apply automatically on start.
- The packages and the Docker image keep projects in `/var/lib/slonbase/projects` and
  `/data/projects`.
- Creating projects needs the database role to have `CREATEDB`, and `CREATEROLE` for projects
  with an own role: `ALTER ROLE slonbase CREATEDB CREATEROLE;`. Nothing changes until you
  create a project.
- The main installation opens one more `LISTEN` connection to its database, and each project
  adds up to `projects.max_connections` (4) plus one.

## [0.1.0] - 2026-10-17

The first release: a PocketBase-compatible backend on PostgreSQL.

### Server and setup

- A single static binary for Linux (amd64, arm64). It needs a PostgreSQL 14+ server, which isn't
  bundled.
- A first-run setup wizard (behind a one-time token) that tests the connection, creates the
  system schema and the first superuser, and switches to normal mode without a restart. Headless
  setup is available with `slonbase setup`.
- Configuration through flags, `SLONBASE_*` environment variables and a TOML file. Application
  settings live in the database and apply to every instance at once.
- System migrations run in one transaction under an advisory lock.
- Graceful stops and restarts that don't wait on realtime connections.
- Packages: `.deb` and `.rpm` with a hardened systemd service, a `.tar.gz`, and a multi-arch
  Docker image at `ghcr.io/ayeshrodz/slonbase`. Docker Compose runs it together with PostgreSQL.
- Commands: `serve`, `setup`, `health`, `superuser upsert|update|delete|otp`, and
  `import-pocketbase`.

### Collections and records

- Base, auth and view collections. Each is stored as a real PostgreSQL table in the `slonbase`
  schema.
- Field types: text, editor, number, bool, email, url, date, autodate, select, json, relation,
  file, geoPoint, password.
- Indexes, including unique and partial (`WHERE`) indexes.
- PocketBase's records API: list (paging, sorting, filters, `fields`, `skipTotal`), view,
  create, update, delete.
- API rules and filters with PocketBase's syntax:
  - relation paths, `@request.*`, `@collection.*` lookups, date macros and JSON paths;
  - the `:each`, `:length`, `:lower` and `:isset` modifiers, and `geoDistance`.
- `expand` for relations and back-relations, up to 6 levels, checked against each collection's
  rules.
- Batch requests (`/api/batch`) that run in one transaction.
- Collection import and export, and truncating a collection.
- PocketBase's validation error codes.

### Auth

- Superusers (`_superusers`) and auth collections.
- Sign-in methods:
  - password;
  - OAuth2: Google, GitHub, GitLab, Discord, Microsoft, Facebook and OpenID Connect, with PKCE;
  - one-time passwords;
  - MFA.
- Email verification, password reset and email change, plus impersonation.
- New-device sign-in alerts.

### Realtime and files

- Realtime subscriptions using PocketBase's SSE protocol. They support filters, `expand` and
  `fields`, and respect each subscriber's rules. Events reach every instance through PostgreSQL
  `LISTEN/NOTIFY`.
- File fields:
  - stored on disk or in any S3-compatible bucket;
  - MIME detection from content, and thumbnails;
  - protected files with file tokens;
  - HTTP range requests.

### Operations

- The admin dashboard at `/_/`: collections, the record grid and editor, the schema editor,
  rules, API previews, logs, the SQL console (off by default), and settings.
- Request and error logs, kept in PostgreSQL.
- Rate limits, superuser IP restrictions, and trusted proxy headers.
- Backups (manual and cron, on disk or in S3) and restores that apply to every instance.
- SMTP mail with PocketBase's templates.

### Extending

- JavaScript hooks (`*.pb.js`) with PocketBase's JSVM API:
  - record request, model, validation and after-success hooks;
  - collection and mail hooks;
  - `routerAdd` and `routerUse`, `cronAdd`;
  - `$app`, `$http`, `$security`, `$filesystem` and `$os`.
- A pool of QuickJS runtimes, with a restart when the hook files change.
- A Rust library API: hooks, routes, cron jobs and a record service, for building a custom
  binary.

### Migration

- `slonbase import-pocketbase` imports PocketBase 0.23+ data (a `pb_data` directory or a backup
  zip) without changing it:
  - collections, records, password hashes, superusers and OAuth2 links;
  - files and settings.

### Compatibility and performance

- A compatibility suite runs the official PocketBase JavaScript SDK against slonbase and against
  PocketBase 0.40.5 in CI.
- Benchmarks against PocketBase are in [bench/README.md](https://github.com/ayeshrodz/slonbase/blob/main/bench/README.md).

[Unreleased]: https://github.com/ayeshrodz/slonbase/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/ayeshrodz/slonbase/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ayeshrodz/slonbase/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ayeshrodz/slonbase/releases/tag/v0.1.0

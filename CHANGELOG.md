# Changelog

All notable changes to slonbase are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). Before 1.0, a minor version (0.x) may contain breaking
changes, and they'll be listed here.

## [Unreleased]

### Added

- **Projects:** one installation can host several isolated projects. Each runs in a PostgreSQL
  database of its own, on the same server, and is served under `/p/<name>/`, dashboard
  included. A project has its own collections, superusers, settings, files, backups, logs and
  hooks.
  - Superusers of the main installation create, list and delete projects with `/api/projects`.
  - Every instance serves every project, and changes reach all of them.
  - See [docs/projects.md](https://github.com/ayeshrodz/slonbase/blob/main/docs/projects.md).
  - Deleting a project backs it up first, and keeps the backup with its files.
  - `slonbase project list|create|delete` on the command line. `--project <name>` runs
    `superuser` and `import-pocketbase` on a project, for example to move a PocketBase app into
    one.
- `projects.path` (`--projects-path`) and `projects.max_connections` settings.

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

[Unreleased]: https://github.com/ayeshrodz/slonbase/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ayeshrodz/slonbase/releases/tag/v0.1.0

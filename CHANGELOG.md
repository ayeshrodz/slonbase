# Changelog

All notable changes to slonbase are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). Before 1.0, a minor version (0.x) may contain breaking
changes, and they'll be listed here.

## [Unreleased]

### Added

- **Realtime, file, settings, backup and terminate hooks**, in JavaScript and Rust:
  `onRealtimeConnectRequest`, `onRealtimeSubscribeRequest`, `onRealtimeMessageSend`,
  `onFileDownloadRequest`, `onFileTokenRequest`, `onSettingsListRequest`,
  `onSettingsUpdateRequest`, `onSettingsReload`, `onBackupCreate`, `onBackupRestore` and
  `onTerminate`, with PocketBase's event properties. The compatibility suite runs the same
  handlers in PocketBase and slonbase. See the [differences](docs/migrating-from-pocketbase.md)
  for the two that behave differently.
- **Collection request and batch hooks:** `onCollectionsListRequest`, `onCollectionViewRequest`,
  `onCollectionCreateRequest`, `onCollectionUpdateRequest`, `onCollectionDeleteRequest`,
  `onCollectionsImportRequest` and `onBatchRequest`, also checked against PocketBase.
- **`$dbx` and the query builder:** `$app.db()` has PocketBase's dbx builder (`select`,
  `where`, joins, `groupBy`, `union`, `insert`, `upsert`, `update`, `delete`, the schema
  methods), `$dbx` its expressions, and `$app.recordQuery(collection)` reads records with it.
  The compatibility suite runs the same queries on PocketBase and slonbase.
- `$app.db().newQuery()` quotes `{{table}}` and `[[column]]` names, as PocketBase does.
- `$app.createBackup(name)` and `$app.restoreBackup(name)`.
- Realtime connections can have an idle and a maximum timeout, set by a connect hook.

### Fixed

- `$app.db().newQuery()` finds collections by their unqualified names (`SELECT * FROM posts`),
  as the extending guide's example does; they needed the `slonbase.` schema before.
- `$app.db().newQuery()` is faster: it compiled its regular expressions on every call.

## [0.4.2] - 2026-10-26

New installations start with rate limits on, and dependencies with security advisories are
updated. Upgrading is recommended.

### Security

- Updated dependencies with RustSec advisories: `rustls` 0.23.46 (four advisories, among them
  a ServerHello downgrade and a decompression bomb), `rustls-webpki` 0.103.16 and
  `webpki-roots`/`webpki-root-certs` 1.0.11 (distrusted certificate authorities), `ctutils`
  0.4.6 (non-constant-time comparisons) and `tokio` 1.54.1 (unsound `AsyncFd` constructors).
- CI runs `cargo audit`, and a release waits for it, so a dependency with a known advisory
  can't be released.

### Changed

- **New installations start with rate limits on**, with the default rules (`*:auth` 2 requests
  per 3 seconds, `*:create` 20 per 5 seconds, `/api/batch` 3 per second, `/api/` 300 per 10
  seconds). PocketBase starts with them off. Existing installations, and imports from
  PocketBase, keep their setting.

## [0.4.1] - 2026-10-23

A security release: everyone on 0.4.0 should upgrade. It also adds ready-made setups for
hosting platforms.

### Security

Fixes from a [review of the authentication code](docs/security-review.md):

- The OTP attempt limit (5 wrong passwords) held for guesses sent one at a time only: guesses
  sent at the same time could all be checked. Each guess now takes an attempt before it's
  checked.
- Behind a proxy that adds its own `X-Forwarded-For` line rather than appending to the client's
  (e.g. HAProxy's `option forwardfor`), a client could choose the IP address that rate limits,
  `superuserIPs` and logs see. Every line of a trusted proxy header is now read.
- OAuth2 avatar downloads only reach public addresses (or the provider's own hosts), so a user
  who can set their avatar URL at the provider can't make slonbase fetch internal addresses.

### Added

- Ready-made setups for hosting platforms ([docs/platforms.md](docs/platforms.md)): a Render
  Blueprint (`render.yaml`, with a Deploy to Render button), Fly.io (`deploy/fly/`), Coolify and
  Dokploy templates (`deploy/coolify/`, `deploy/dokploy/`), and the steps for Railway.

## [0.4.0] - 2026-10-21

**Hooks parity:** most of what `pb_hooks` files use is now there: PocketBase's auth request
hooks, `onRecordEnrich`, `$template`, `$mails` and the record token methods. The
compatibility suite runs the same hooks in PocketBase and slonbase and checks they behave
alike.

### Added

- **Auth request hooks**, from JavaScript and Rust (`Hooks::on_record_auth_request`, ...):
  - `onRecordAuthRequest`: every sign-in, token refresh and impersonation, before the MFA
    check. Handlers can change the record in the response, `e.token` and `e.meta`, or throw.
  - `onRecordAuthWithPasswordRequest`, `onRecordAuthWithOAuth2Request`,
    `onRecordAuthWithOTPRequest` and `onRecordAuthRefreshRequest`, with the hook's data
    (`e.identity`, `e.password`, `e.oauth2User`, `e.createData`, `e.otp`, ...). Handlers can
    find the record their own way.
  - `onRecordRequestOTPRequest`, and the request and confirm hooks of verifications, password
    resets and email changes. A handler that doesn't call `e.next()` skips the action.
- **`onRecordEnrich`**: every record sent to a client (responses, batches, auth responses,
  expanded relations, realtime messages). Handlers can add keys and `hide()` or `unhide()`
  fields.
- **`$template`**: Go's `html/template` language, with `loadFiles()`, `loadString()` and
  `addFuncs()`. Output is HTML-escaped, but not by context like Go's: use `js` and `urlquery`
  inside scripts and URLs.
- **`$mails`**: `sendRecordVerification`, `sendRecordPasswordReset`, `sendRecordChangeEmail`,
  `sendRecordOTP` and `sendRecordAuthAlert`.
- Record token methods: `newVerificationToken()`, `newPasswordResetToken()`,
  `newEmailChangeToken()`, `newFileToken()` and `newStaticAuthToken()`.

### Fixed

- A panic in a JavaScript native function fails that call instead of stopping its runtime.

### Upgrading from 0.3

Install the new version as usual. Nothing changes in the database, and hooks written for 0.3
keep working.

## [0.3.0] - 2026-10-20

**Migrations:** slonbase runs JavaScript migrations, PocketBase's `pb_migrations` included, so
collections can live in code and reach every installation the same way. Each migration runs
in a transaction of its own. See
[docs/migrations.md](https://github.com/ayeshrodz/slonbase/blob/v0.3.0/docs/migrations.md).

### Added

- **JavaScript migrations**, compatible with PocketBase's `pb_migrations`: `*.js` files in the
  migrations directory (`--migrations-dir`, `slonbase_migrations` by default) that call
  `migrate(up, down)`.
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

### Upgrading from 0.2

Install the new version as usual (with the package repositories, `apt upgrade` or
`dnf upgrade`).

- System migration 9 (the list of applied JavaScript migrations) applies on start.
- Nothing runs until there are migration files. The packages look for them in
  `/var/lib/slonbase/migrations`, the Docker image in `/data/migrations`, and projects in
  `<projects.path>/<name>/migrations`.
- Backups made with 0.2 restore as before; they leave the list of applied migrations alone.

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

[Unreleased]: https://github.com/ayeshrodz/slonbase/compare/v0.4.2...HEAD
[0.4.2]: https://github.com/ayeshrodz/slonbase/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/ayeshrodz/slonbase/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/ayeshrodz/slonbase/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/ayeshrodz/slonbase/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/ayeshrodz/slonbase/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ayeshrodz/slonbase/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ayeshrodz/slonbase/releases/tag/v0.1.0

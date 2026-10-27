# Changelog

All notable changes to slonbase are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). Before 1.0, a minor version (0.x) may contain breaking
changes, and they'll be listed here.

## [Unreleased]

### Added

- **Soak tests** (`soak/`): sustained mixed traffic on several instances sharing a database,
  failing on request errors, missed or late realtime events, memory growth, latency drift or
  leaked connections. CI runs them weekly.
- **A custom CA for S3 and other outbound HTTPS:** `--http-ca-file` (`SLONBASE_HTTP_CA_FILE`,
  `server.http_ca_file`) trusts a PEM bundle's certificates besides the public roots, for S3
  servers (and OAuth2 providers) with certificates from a private CA.
- **Encrypted backups:** with a backups encryption key (`SLONBASE_BACKUPS_ENCRYPTION_KEY`,
  `backups.encryption_key`), new backups are encrypted with AES-256-GCM before they're stored,
  locally or in S3, and anything changed or cut short fails to restore. Restores and uploads
  decrypt them; a plain backup uploaded to such an installation is stored encrypted.
  `slonbase decrypt-backup` turns one back into its zip.
- **Multipart S3 uploads:** files over 64 MiB, such as backups, are uploaded in parts, each
  retried on its own, so large backups no longer depend on one long request and can pass the
  5 GiB a single S3 upload allows. A failed upload is aborted, leaving no parts behind.

### Fixed

- **Realtime keeps up with sustained writes:** events were checked and sent one at a time, so
  under heavy writes their delivery fell seconds behind, and further each minute. They're now
  read in batches and checked several at a time (still in order for each client): in the soak
  test, from a p50 of 7 s (and growing) to 13 ms.

## [0.7.0] - 2026-10-27

**Parity details, and Sign in with Apple.** Filters and rules gain `strftime()`, reproduced
exactly from SQLite (PocketBase's) and checked against it on tens of thousands of cases, while
staying fast on date fields. Sign in with Apple, with its client secret generator, and OpenID
Connect providers read from a validated `id_token`. Removing a file field deletes its files,
and `logs.maxDataSize` works. Also a new identity: slonbase now describes itself as what it is,
a backend built from scratch for PostgreSQL.

### Added

- **`strftime()` in filters and rules:** `strftime(format, [time-value, modifiers...])`
  formats a date as SQLite's strftime does, as in PocketBase, e.g.
  `strftime('%Y-%m', created) = '2026-01'`. Every format code, time value and modifier
  behaves as in SQLite 3.53 (PocketBase 0.40's), checked against it on tens of thousands of
  cases. On date fields the common formats compile to PostgreSQL's `to_char()`, so filtering
  stays fast. Function calls now take up to 10 arguments.
- **`logs.maxDataSize` works:** log entries whose data is larger keep the attributes that fit
  and are marked with `"__pb_truncated__": true`, as in PocketBase (16 KiB when it's 0), and
  messages are cut at 8,000 bytes. The dashboard's log settings have the field. Values below 0
  are rejected.
- **Sign in with Apple:** an `apple` OAuth2 provider, as in PocketBase. The user comes from
  Apple's `id_token`, verified with Apple's keys, and the name Apple sends only with the first
  redirect is kept for the sign-in (in the database, so it works across instances).
  `POST /api/settings/apple/generate-client-secret` signs the client secret Apple requires, and
  the dashboard's Apple provider form has a generator for it.
- **OpenID Connect sign-in from the `id_token`:** an `oidc` provider without a user info URL
  reads the user from the token response's `id_token`, as in PocketBase. The audience and
  times are checked; the `jwksURL` extra option verifies the signature (RSA or Ed25519 keys)
  and `issuers` limits the accepted issuers. The dashboard's provider form has both.
- **Removing a file field deletes its files:** when a collection change (through the API, an
  import, a hook or a migration) removes a file field, the field's stored files and their
  thumbnails are deleted once the change commits. Before, they stayed in storage with nothing
  referring to them (as they do in PocketBase). Renaming a field keeps them.

### Changed

- **slonbase on its own terms:** the README, the landing page, the docs, the package
  descriptions and `slonbase --help` now describe slonbase as what it is, a backend built from
  scratch for PostgreSQL ("Your whole backend, built on PostgreSQL"), rather than as
  PocketBase's API on another database. Compatibility with the PocketBase SDKs, hooks and
  migrations is unchanged, and is described in one place for people moving an app over.
- The docs are up to date with 0.6 (the design document's roadmap, stale version notes).
- OpenID Connect providers no longer need a user info URL (they're read from the `id_token`
  without one).

### Upgrading

- Two system migrations run on the first start: the `strftime()` functions, and a small
  unlogged table for the name Apple sends with its first redirect. Back up first, as for every
  upgrade.
- Removing a file field from a collection now deletes the field's stored files. Before, they
  were left in storage (with no way to reach them once the column was gone).
- Log entries larger than `logs.maxDataSize` (16 KiB by default) are now truncated, and messages
  are cut at 8,000 bytes.

## [0.6.0] - 2026-10-26

**Realtime for every change, and many instances behind any load balancer.** Realtime events now
come from the database itself, so changes made outside the records API (SQL, auth flows, other
programs) reach subscribers. Several instances can sit behind a plain round-robin load
balancer: realtime subscriptions find the client's instance, and rate limits are shared.
Also `@request.auth` relation paths, Prometheus metrics, and an upgrade test in CI.

### Added

- **Realtime for every change**, not only the records API's: auth flows (verifying an email,
  resetting a password, changing an email), SQL from hooks, migrations and the SQL console, and
  other programs writing to the collection tables now reach subscribers. A trigger on each
  table writes the event in the change's transaction (rolled-back changes still send nothing);
  existing tables get it when the upgrade runs. `SET slonbase.realtime = off` skips events in a
  session, for bulk loads.
- **Several instances behind any load balancer:** realtime no longer needs client affinity
  (a subscribe request reaching another instance than the client's stream is forwarded to it
  through PostgreSQL), and rate limits count the requests of all instances (in an unlogged
  table, only while more than one instance is live).
- **`@request.auth` relation paths** in rules and filters, as in PocketBase:
  `@request.auth.team.owner = @request.auth.id`, `@request.auth.groups.name ?= 'editors'`,
  `@request.auth.team.settings.open = true`. Checked against PocketBase.
- **Prometheus metrics** at `/metrics` on their own address (`--metrics-bind`, off by default):
  requests and durations by route pattern, database pool use, realtime clients and events,
  instances. See [monitoring](https://github.com/ayeshrodz/slonbase/blob/v0.6.0/docs/deployment.md#monitoring).
- CI tests upgrades in place from 0.1.0 and from the previous release.

### Fixed

- Email fields accept what PocketBase accepts: underscores inside domain labels
  (`a@my_host.example`) and a trailing dot; they now reject labels that start or end with `-`,
  empty labels (`x..com`) and IP addresses, as PocketBase does.

### Upgrading

- Two system migrations run on the first start: a realtime trigger on every collection table
  (each table is locked for a moment while it's added) and the tables for instances, forwarded
  subscriptions and shared rate limits. Back up first, as for every upgrade.
- Subscribers may get events they didn't before: for changes made with SQL or by other
  programs, and for auth flows such as email verification. Bulk SQL writes now produce one event
  per row; use `SET slonbase.realtime = off` in that session to skip them.
- With several instances, rate limits now apply across them, so a client gets the configured
  limit once, not once per instance. Raise the limits if you sized them for that.
- Records with an email that PocketBase rejects (such as `a@-x.com`) fail validation the next
  time they're saved.

## [0.5.0] - 2026-10-26

**Every PocketBase hook:** slonbase now has every hook PocketBase's JavaScript declares, from
the realtime and file hooks to `onModel*` and the execute and after-error hooks, plus the
`$dbx` query builder. The compatibility suite runs the same hooks and queries in PocketBase
and slonbase and checks they behave alike (48 tests). The few intended differences are listed
in [the migration guide](https://github.com/ayeshrodz/slonbase/blob/v0.5.0/docs/migrating-from-pocketbase.md).

### Added

- **Realtime, file, settings, backup and terminate hooks**, in JavaScript and Rust:
  `onRealtimeConnectRequest`, `onRealtimeSubscribeRequest`, `onRealtimeMessageSend`,
  `onFileDownloadRequest`, `onFileTokenRequest`, `onSettingsListRequest`,
  `onSettingsUpdateRequest`, `onSettingsReload`, `onBackupCreate`, `onBackupRestore` and
  `onTerminate`, with PocketBase's event properties. The compatibility suite runs the same
  handlers in PocketBase and slonbase. See the [differences](https://github.com/ayeshrodz/slonbase/blob/v0.5.0/docs/migrating-from-pocketbase.md)
  for the two that behave differently.
- **Collection request and batch hooks:** `onCollectionsListRequest`, `onCollectionViewRequest`,
  `onCollectionCreateRequest`, `onCollectionUpdateRequest`, `onCollectionDeleteRequest`,
  `onCollectionsImportRequest` and `onBatchRequest`, also checked against PocketBase.
- **Execute and after-error hooks:** `onRecordCreateExecute`, `onRecordUpdateExecute` and
  `onRecordDeleteExecute` wrap the write itself (code after `e.next()` runs after it, in the
  transaction); `onRecordAfterCreateError`, `onRecordAfterUpdateError` and
  `onRecordAfterDeleteError` report failed writes, including ones a failed batch undid. The
  collection versions (`onCollectionCreateExecute`, `onCollectionAfterCreateError`, ...),
  `onCollectionValidate` and `onBootstrapClear` too. Checked against PocketBase.
- **Generic model hooks:** `onModelCreate`, `onModelUpdate`, `onModelDelete`,
  `onModelValidate`, their `Execute` versions and `onModelAfter{Create,Update,Delete}{Success,Error}`,
  for records and collections (`e.model`), in PocketBase's order. With these, slonbase has every
  hook PocketBase's JavaScript has.
- `toBytes`, `readerToString` and `nullString()`/`nullInt()`/... for `DynamicModel` columns.
- **`$dbx` and the query builder:** `$app.db()` has PocketBase's dbx builder (`select`,
  `where`, joins, `groupBy`, `union`, `insert`, `upsert`, `update`, `delete`, the schema
  methods), `$dbx` its expressions, and `$app.recordQuery(collection)` reads records with it.
  The compatibility suite runs the same queries on PocketBase and slonbase.
- `$app.createBackup(name)` and `$app.restoreBackup(name)`.
- Realtime connections can have an idle and a maximum timeout, set by a connect hook.

### Changed

- `$app.db().newQuery()` runs with the search path `slonbase, public`, so unqualified names
  find collections first, then tables in `public`. A query naming a `public` table that
  shares its name with a collection now reads the collection; qualify it (`public.name`).
- `{{name}}` and `[[name]]` in `newQuery()` SQL are quoted as names, as in PocketBase.

### Fixed

- `$app.db().newQuery()` finds collections by their unqualified names (`SELECT * FROM posts`),
  as the extending guide's example does; they needed the `slonbase.` schema before.
- `$app.db().newQuery()` is faster: it compiled its regular expressions on every call.
- `toString()` of bytes decodes UTF-8 (it read them as Latin-1, so non-ASCII text came out
  garbled).

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

[Unreleased]: https://github.com/ayeshrodz/slonbase/compare/v0.7.0...HEAD
[0.7.0]: https://github.com/ayeshrodz/slonbase/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/ayeshrodz/slonbase/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/ayeshrodz/slonbase/compare/v0.4.2...v0.5.0
[0.4.2]: https://github.com/ayeshrodz/slonbase/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/ayeshrodz/slonbase/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/ayeshrodz/slonbase/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/ayeshrodz/slonbase/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/ayeshrodz/slonbase/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ayeshrodz/slonbase/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ayeshrodz/slonbase/releases/tag/v0.1.0

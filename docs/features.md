# Features

What slonbase does, by area, with a link to the guide for each. slonbase is a self-hosted backend
platform on PostgreSQL: one Rust binary, with the dashboard built in.

## Data and API

- **Collections as real tables.** Base, auth and view collections with 14 field types, indexes
  (unique and partial ones too) and relations. Each collection is a table in the `slonbase`
  schema: query it with SQL, join it, replicate it. See the [API guide](api.md).
- **A REST API** with filtering, sorting, paging, `expand` for relations, field selection,
  batch requests in one transaction and clear, field-level validation errors.
- **One filter language** for queries and for access rules, compiled to parameterized SQL that
  uses your indexes: relation paths, `@request.*`, `@collection.*`.
- **View collections** from an SQL query, with a live preview while you write it.
- **Migrations** in JavaScript, each in a transaction, with automigrate ([migrations](migrations.md)).

## Authentication and access

- Password, **OAuth2** (dozens of providers and any OpenID Connect provider), one-time passwords
  and multi-factor sign-in; verification, password reset and email change; new-device alerts and
  impersonation.
- **Roles for superusers:** Owner, Admin, Developer and Viewer, checked on every request, and
  **API keys** for scripts and CI, each with a role, an expiry date and allowed IP addresses.
- **Per-collection API rules,** lockout after failed sign-ins, rate limits with a tester that says
  which rule a request would meet, CORS, superuser IP restrictions and a maintenance mode.

## Realtime

- Subscriptions over SSE with per-subscriber rules, filters and `expand`.
- **Events for every change to a table, whoever makes it:** the API, your own SQL, an auth flow
  or another program. A trigger on each table announces the write.

## Files

- File fields on disk or any S3-compatible bucket, with thumbnails, protected files and byte
  ranges; Settings shows where files are, how much room there is and what they take.

## The dashboard

- Live record grids, a record editor in a drawer, a schema editor and diagram, API rules, an API
  preview per collection, request logs and the health of the installation.
- **An SQL console** that only reads unless you allow writes, with limits, a schema browser,
  per-installation history and exports.
- **A command center for many projects:** requests per minute, errors, latency, data and
  configuration of every installation on a server (or several), side by side.
- **Make it yours:** thirteen named accents or a colour of your own, light and dark mode, the
  application's name and logo everywhere, **a sign-in page you lay out yourself** and **an email
  designer** with a layout, placeholders and a live preview. Each project has its own.

## Projects and scale

- **Many projects, one server:** each project is an isolated installation with its own users,
  superusers, collections, settings and files, in a PostgreSQL database of its own, on the main
  server or another ([projects](projects.md)).
- **Several instances on one database,** behind any load balancer, with no sticky sessions, and optionally behind PgBouncer. Tested layouts, the connection budget and limits are in the [scaling guide](scaling.md).
  Realtime, schema changes, settings and rate limits are shared through PostgreSQL
  ([deployment](deployment.md)).

## Operations

- Request logs, Prometheus metrics, and a health page.
- **Backups** on a schedule or on demand, to disk or S3, optionally **encrypted**, with one-click
  restore, and **a history of every backup and cron job run**: when, how long, how it ended.
- A one-command **installer**, `.deb` and `.rpm` packages from signed repositories, a Docker image
  and Compose file, and setups for Render, Railway, Fly.io, Coolify and Dokploy
  ([installation](installation.md), [platforms](platforms.md)).
- Upgrades tested in place from every release, and a [stability promise](stability.md).

## Extending

- **JavaScript hooks,** routes, cron jobs, migrations and a query builder, with no build step;
  PocketBase's hooks run as they are ([extending](extending.md)).
- **A Rust library API** to embed slonbase in your own program.

## Compatibility

- The **PocketBase JavaScript and Dart SDKs** work against slonbase's API, and
  `slonbase import-pocketbase` brings over an existing PocketBase installation, password hashes
  included ([migrating](migrating-from-pocketbase.md), [slonbase and PocketBase](slonbase-and-pocketbase.md)).

## Requirements

PostgreSQL 14 or newer (not bundled), and Linux on x86_64 or aarch64 for the release builds.
The [quickstart](quickstart.md) takes a bare server to a working backend in a few minutes.

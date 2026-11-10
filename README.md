<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-dark.svg">
    <img alt="slonbase" src="docs/assets/banner-light.svg" width="640">
  </picture>
</p>

<p align="center">
  <strong>Your whole backend, built on PostgreSQL.</strong><br>
  slonbase is an open-source backend platform on PostgreSQL: a REST and realtime API, auth with
  roles, files, hooks and an admin dashboard for many projects, in one Rust binary.
  Every collection is a real PostgreSQL table.
</p>

<p align="center">
  <a href="https://github.com/ayeshrodz/slonbase/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/ayeshrodz/slonbase/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://github.com/ayeshrodz/slonbase/releases"><img alt="Release" src="https://img.shields.io/github/v/release/ayeshrodz/slonbase?display_name=tag&sort=semver&color=4f7a6c"></a>
  <a href="https://github.com/ayeshrodz/slonbase/pkgs/container/slonbase"><img alt="Docker image" src="https://img.shields.io/badge/ghcr.io-slonbase-4f7a6c?logo=docker&logoColor=white"></a>
  <img alt="PostgreSQL 14+" src="https://img.shields.io/badge/PostgreSQL-14%2B-336791?logo=postgresql&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-1.89%2B-b7410e?logo=rust&logoColor=white">
  <a href="LICENSE"><img alt="Apache 2.0 license" src="https://img.shields.io/badge/license-Apache--2.0-4f7a6c"></a>
</p>

<p align="center">
  <a href="https://slonbase.com/">Website</a> ·
  <a href="https://slonbase.com/docs/features/">Features</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="docs/installation.md">Install</a> ·
  <a href="docs/api.md">API</a> ·
  <a href="docs/extending.md">Hooks</a> ·
  <a href="bench/README.md">Benchmarks</a>
</p>

<p align="center">
  <img alt="A tour of the slonbase dashboard: signing in, the overview and a live records grid" src="docs/assets/demo.webp" width="900">
</p>

<p align="center">
  <sub>A JavaScript client creating and updating a task. The dashboard's grid is live: realtime events arrive as the code runs.</sub>
</p>

## Install

- **One command on a bare Linux server:** `curl -fsSL https://slonbase.com/install.sh | sudo sh`
  installs PostgreSQL if needed, creates its role and database, installs slonbase and creates
  your first superuser. The [installation guide](docs/installation.md) does every step by hand,
  and covers packages, Docker and building from source.
- **What changed lately:** see the [changelog](CHANGELOG.md) and the
  [releases](https://github.com/ayeshrodz/slonbase/releases).
- **More:** the [tour of the dashboard](https://slonbase.com/), [benchmarks](bench/README.md)
  against PocketBase and PostgREST, and every [feature](https://slonbase.com/docs/features/) in
  one list.

## Why slonbase

Define your data in a dashboard and get an API, auth, realtime and file storage right away,
without writing a backend. slonbase gives you that in a single binary, runs as many isolated
projects as you like from one dashboard, and keeps your data in **PostgreSQL**, the database you
can query, replicate, back up and scale with the tools you already trust.

- 🐘 **Real PostgreSQL tables.** Every collection is a table. Query it with SQL, join it in
  reports, replicate it, back it up with `pg_dump`, or run it on a managed service.
- ⚡ **Realtime for every change.** A trigger on each table announces every write, whoever makes
  it: the API, your SQL, an auth flow or another program. Each subscriber gets only what its
  rules allow.
- ↔️ **Scales out.** Run several instances on one database behind any load balancer. Realtime,
  schema changes, settings and rate limits are shared through PostgreSQL, so no sticky sessions.
  Tested layouts (a balancer, PgBouncer, projects on other servers) and the connection budget are
  in the [scaling guide](docs/scaling.md).
- 🧩 **Extend it your way.** Hooks, routes, cron jobs and a query builder in JavaScript files,
  no build step; or a Rust library API.
- 🗂️ **Many projects, one server.** Host several isolated apps from one installation, each in a
  database of its own, with a command center for all of them.
- 🎛️ **A dashboard that is yours.** An SQL console, roles and API keys for your team, an email
  designer and a sign-in page you lay out yourself, with your name, colours and logo.
- 🚀 **Fast where it matters.** Written in Rust with tokio and axum; filters compile to
  parameterized SQL that uses your indexes ([benchmarks](bench/README.md)).
- 📦 **One static binary.** A 10 MB download (22 MB on disk) with the dashboard built in. Install it with
  Docker, `.deb`/`.rpm` packages or a plain binary, and finish with a setup wizard.

## Features

<table>
<tr>
<td width="50%" valign="top">

**Data**
- Base, auth and view collections, with 14 field types; live previews of views
- A filter language for queries and access rules: relation paths, `@request.*`, `@collection.*`
- `expand`, back-relations, sorting and paging
- `fields` with nested paths and plain-text `:excerpt`s of HTML, as in PocketBase
- Batch requests in one transaction
- Indexes, including unique and partial ones

**Auth**
- Password, OAuth2 (Google, GitHub, Microsoft, OIDC, …), one-time passwords and MFA
- Verification, password reset and email change
- New-device sign-in alerts, impersonation

</td>
<td width="50%" valign="top">

**Realtime and files**
- SSE subscriptions with per-subscriber rules, filters and `expand`
- Events for every change, made through the API or not
- File fields on disk or any S3-compatible bucket; thumbnails, protected files, byte ranges

**Operations**
- An admin dashboard with live grids, a schema editor and diagram, logs and an SQL console
- Request logs, rate limits, superuser IP restrictions, Prometheus metrics
- gzip compression of JSON that never touches tokens
- Backups (on a schedule, to S3) and one-click restores
- JavaScript hooks, cron jobs, a query builder, and a Rust library API
- Migrations in JavaScript, each in a transaction, with automigrate

</td>
</tr>
</table>

> [!NOTE]
> **slonbase is stable:** within a major version, upgrades don't break the API, configuration,
> your data, hooks or migrations (see [stability](docs/stability.md)). Back up before upgrading anyway, and
> bug reports are very welcome.

## Already using PocketBase?

slonbase is its own platform (an independent codebase, written from scratch in Rust for
PostgreSQL), and its API stays compatible with [PocketBase](https://pocketbase.io)'s, the project
that inspired it, so you can bring an existing app along. See also
[how the two differ](https://slonbase.com/docs/slonbase-and-pocketbase/):

- the PocketBase JavaScript and Dart SDKs work against slonbase as they are;
- most `pb_hooks` and `pb_migrations` files run unchanged;
- `slonbase import-pocketbase` brings over collections, records, files and settings, with users'
  password hashes, so nobody has to reset a password.

A test suite written with the official SDK runs against both servers in CI to keep it that way,
and a differential runner sends both the same requests and compares the answers
([compat/](compat/)). The [migration guide](docs/migrating-from-pocketbase.md) lists the
differences.

PocketBase is the better choice when you want zero moving parts (one file, SQLite inside), when
one small app on one small machine is the whole plan, or when you extend it in Go.

## Quick start

**On a Linux server** (Debian, Ubuntu, Fedora, RHEL, Rocky, Alma): one command installs
PostgreSQL if the machine has none, creates the database and its role, installs slonbase from
the signed repository and creates your first superuser:

```sh
curl -fsSL https://slonbase.com/install.sh | sudo sh
```

It asks for your email and prints the dashboard address and a generated password. The
[installation guide](docs/installation.md) shows every step by hand, from a bare server.

**With Docker.** This starts slonbase and a PostgreSQL 17 server for it:

```sh
mkdir slonbase && cd slonbase
curl -fsSLO https://raw.githubusercontent.com/ayeshrodz/slonbase/main/compose.yaml
echo "POSTGRES_PASSWORD=$(openssl rand -hex 24)" > .env
docker compose up -d
docker compose logs slonbase | grep "setup?token"
```

Open the printed link and create your first superuser. That's it: the dashboard is at
`http://localhost:8090/_/` and the API at `/api/`.

**On a hosting platform:** [Render](docs/platforms.md#render) (one click), [Railway](docs/platforms.md#railway),
[Fly.io](docs/platforms.md#flyio), [Coolify](docs/platforms.md#coolify) and
[Dokploy](docs/platforms.md#dokploy) each have a ready-made setup with a database.

<a href="https://render.com/deploy?repo=https://github.com/ayeshrodz/slonbase"><img alt="Deploy to Render" src="https://render.com/images/deploy-to-render-button.svg" height="32"></a>

**Other ways:** the `.deb`, `.rpm` and static binaries are on the
[releases](https://github.com/ayeshrodz/slonbase/releases) page, and the apt and dnf
repositories are described in the [installation guide](docs/installation.md#3-install-slonbase).
PostgreSQL isn't bundled: slonbase connects to any 14+ server, on the same machine, elsewhere,
or a managed service.

Then connect from your app with any HTTP client, or with the PocketBase SDKs, which work with
slonbase's API:

```js
import PocketBase from "pocketbase";

const pb = new PocketBase("http://localhost:8090");
await pb.collection("users").authWithPassword("ann@example.com", "secret");
const posts = await pb.collection("posts").getList(1, 20, { filter: 'published = true', expand: "author" });
```

## A look around

| Records | Overview |
|---|---|
| <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/records-dark.png"><img alt="A collection's records in a live grid" src="docs/assets/screenshots/records-light.png"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/overview-dark.png"><img alt="Overview" src="docs/assets/screenshots/overview-light.png"></picture> |
| **Schema** | **Logs** |
| <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/schema-dark.png"><img alt="Schema editor" src="docs/assets/screenshots/schema-light.png"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/logs-dark.png"><img alt="Logs" src="docs/assets/screenshots/logs-light.png"></picture> |
| **Projects: the command center** | **SQL console** |
| <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/projects-dark.png"><img alt="Every installation side by side: requests per minute, errors, latency, data and configuration" src="docs/assets/screenshots/projects-light.png"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/sql-dark.png"><img alt="The SQL console: tables and columns, a highlighted query and its results, ready to export" src="docs/assets/screenshots/sql-light.png"></picture> |
| **Record editor** | **API rules** |
| <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/editor-dark.png"><img alt="A record open in the drawer beside the grid" src="docs/assets/screenshots/editor-light.png"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/rules-dark.png"><img alt="API rules" src="docs/assets/screenshots/rules-light.png"></picture> |
| **Settings: appearance** | **Email designer** |
| <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/settings-dark.png"><img alt="Settings: color mode, thirteen named accents and a preview" src="docs/assets/screenshots/settings-light.png"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/mail-dark.png"><img alt="The email designer: the layout and each template, with a live preview below" src="docs/assets/screenshots/mail-light.png"></picture> |

<picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/diagram-dark.png"><img alt="The schema diagram: every collection with its fields, and its relations" src="docs/assets/screenshots/diagram-light.png"></picture>
<p align="center"><sub>The schema diagram, drawn from the collections: hover one to follow its relations.</sub></p>

The sidebar holds the sections and your collections, records open in a drawer beside the
grid, and a status bar shows the connection, database and API base; light and dark follow
GitHub's neutral palette (or your system). It works on phones, ⌘K / Ctrl K jumps anywhere, and it only uses the public API, so
anything it does, you can script too.

## Documentation

The guides are also on the documentation site, with search:
**[slonbase.com/docs](https://slonbase.com/docs/)**.

- [Installation](docs/installation.md): Docker, packages, binaries, first-run setup, upgrades
- [Configuration](docs/configuration.md): server options and application settings
- [Scaling](docs/scaling.md): several instances, PgBouncer, projects on several servers, connections
- [Hosting platforms](docs/platforms.md): Render, Railway, Fly.io, Coolify and Dokploy
- [Running in production](docs/deployment.md): reverse proxy, PostgreSQL, several instances,
  backups, monitoring, security
- [API](docs/api.md): endpoints, collections, filters and rules, files, realtime
- [Extending](docs/extending.md): JavaScript hooks and the Rust library
- [Migrations](docs/migrations.md): collections and data changes in code
- [Projects](docs/projects.md): several isolated installations, one database each, on one server
- [Migrating from PocketBase](docs/migrating-from-pocketbase.md): the importer and the differences
- [Benchmarks](bench/README.md): slonbase next to PocketBase on the same workloads
- [Design](docs/design.md): the decisions behind slonbase
- [Stability](docs/stability.md): what 1.0 keeps compatible, deprecations and upgrades
- [Changelog](CHANGELOG.md)

## Commands

| Command | Description |
|---|---|
| `slonbase serve` | Start the server (default). |
| `slonbase setup` | Set up without the web wizard. |
| `slonbase health` | Exit 0 if the local server is healthy (for container health checks). |
| `slonbase superuser upsert\|update\|delete\|otp ...` | Manage superusers. `otp <email>` prints a one-time password for signing in with MFA when emails can't be sent. |
| `slonbase import-pocketbase <pb_data or backup.zip>` | Import a PocketBase installation. |
| `slonbase migrate up\|down\|create\|collections\|list ...` | Apply, revert or write [migrations](docs/migrations.md). |
| `slonbase project list\|create\|delete ...` | Manage [projects](docs/projects.md); `--project <name>` runs `superuser`, `import-pocketbase` and `migrate` on one. |

`slonbase --help` lists every option.

## Performance

Compared with PocketBase and PostgREST, on the same machine, the same data and the same HTTP
workloads, with default settings ([method and full results](bench/README.md)):

| Scenario | slonbase 2.2 req/s | PocketBase 0.40.6 req/s | PostgREST 16.5 req/s | p99 ms (slonbase / PocketBase / PostgREST) |
|---|--:|--:|--:|--:|
| Health check | 44,316 | 32,270 | 36,267 | 2.1 / 6.4 / 2.2 |
| View one record | 19,965 | 11,484 | 5,197 | 3.7 / 23.4 / 14.4 |
| List 30 records | 7,814 | 1,610 | 3,733 | 7.8 / 67.1 / 13.7 |
| List 30, filtered | 7,100 | 105 | 3,315 | 8.2 / 439 / 15.4 |
| List 30, expand a relation | 3,906 | 1,065 | n/a | 12.6 / 93.3 / n/a |
| Create a record | 7,424 (8,722 with async commit) | 4,051 | 3,828 | 8.4 / 45.5 / 16.2 |

32 clients, medians of three 10-second runs on a 4-vCPU VM. slonbase is ahead in every scenario
measured, from 1.2 times PocketBase's throughput (sign-in, which bcrypt bounds) to 68 times (filtered
lists, where PostgreSQL uses the index). PostgREST is a leaner program that does much less (no sign-in,
files, realtime or dashboard): a 15 MB process against slonbase's 40 MB. Writes are ahead of PocketBase
with PostgreSQL's default durability, and further ahead when PostgreSQL makes the same trade-off as
SQLite's default (`synchronous_commit = off`; see [why](docs/deployment.md#postgresql)). Lists are
gzipped, a sixth of the bytes, which this loopback benchmark counts as CPU only. The numbers come from
one machine and will differ on yours: the [bench README](bench/README.md) has the caveats and the commands to repeat them.

## Contributing

Bug reports, ideas and pull requests are welcome; see
[CONTRIBUTING.md](CONTRIBUTING.md). The quickest checks:

```sh
cargo test                                                    # unit tests
SLONBASE_TEST_DATABASE_URL='postgres://postgres:postgres@127.0.0.1:5432/postgres?sslmode=disable' cargo test   # everything
(cd compat && npm ci) && DATABASE_URL='postgres://...' compat/run.sh slonbase                # the SDK compatibility suite
```

Please report security problems [privately](SECURITY.md).

## License

slonbase is licensed under the [Apache License 2.0](LICENSE): free to use, modify, host and sell
services around, with an explicit patent grant. The name and logo are covered by the
[trademark policy](TRADEMARKS.md). Releases up to 2.3.0 were MIT. Contributions are accepted
under a short [CLA](CLA.md) (you keep your copyright). Everything released under Apache 2.0 stays
under it; the maintainer may in future offer separately licensed commercial features or hosted
services, which is what the CLA makes possible. See [license and contributing terms](docs/licensing.md).

The dashboard vendors Preact (MIT) and htm (Apache-2.0); their licenses are in
[ui/app/vendor](ui/app/vendor). It ships the IBM Plex Sans and JetBrains Mono fonts (SIL
Open Font License 1.1); the licences are in [ui/app/fonts](ui/app/fonts). All third-party
notices are in [NOTICE](NOTICE).
slonbase is an independent project inspired by PocketBase (MIT). It is not affiliated
with or endorsed by PocketBase or its author, and it wouldn't exist without their work.

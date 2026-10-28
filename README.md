<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-dark.svg">
    <img alt="slonbase" src="docs/assets/banner-light.svg" width="640">
  </picture>
</p>

<p align="center">
  <strong>Your whole backend, built on PostgreSQL.</strong><br>
  An open-source backend in one Rust binary: collections, a REST and realtime API, auth, files,
  hooks and an admin dashboard. Every collection is a real PostgreSQL table.
</p>

<p align="center">
  <a href="https://github.com/ayeshrodz/slonbase/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/ayeshrodz/slonbase/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://github.com/ayeshrodz/slonbase/releases"><img alt="Release" src="https://img.shields.io/github/v/release/ayeshrodz/slonbase?display_name=tag&sort=semver&color=4a5a72"></a>
  <a href="https://github.com/ayeshrodz/slonbase/pkgs/container/slonbase"><img alt="Docker image" src="https://img.shields.io/badge/ghcr.io-slonbase-4a5a72?logo=docker&logoColor=white"></a>
  <img alt="PostgreSQL 14+" src="https://img.shields.io/badge/PostgreSQL-14%2B-336791?logo=postgresql&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-1.87%2B-b7410e?logo=rust&logoColor=white">
  <a href="LICENSE"><img alt="MIT license" src="https://img.shields.io/badge/license-MIT-4a5a72"></a>
</p>

<p align="center">
  <a href="https://ayeshrodz.github.io/slonbase/">Website</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="docs/installation.md">Install</a> ·
  <a href="docs/api.md">API</a> ·
  <a href="docs/extending.md">Hooks</a> ·
  <a href="bench/README.md">Benchmarks</a>
</p>

<p align="center">
  <img alt="A client app creating and updating a task while the slonbase dashboard's grid updates live" src="docs/assets/demo.gif" width="900">
</p>

<p align="center">
  <sub>A JavaScript client creating and updating a task. The dashboard's grid is live: realtime events arrive as the code runs.</sub>
</p>

## Why slonbase

Define your data in a dashboard and get an API, auth, realtime and file storage right away,
without writing a backend. slonbase gives you that in a single binary, and keeps your data in
**PostgreSQL**, the database you can query, replicate, back up and scale with the tools you
already trust.

- 🐘 **Real PostgreSQL tables.** Every collection is a table. Query it with SQL, join it in
  reports, replicate it, back it up with `pg_dump`, or run it on a managed service.
- ⚡ **Realtime for every change.** A trigger on each table announces every write, whoever makes
  it: the API, your SQL, an auth flow or another program. Each subscriber gets only what its
  rules allow.
- ↔️ **Scales out.** Run several instances on one database behind any load balancer. Realtime,
  schema changes, settings and rate limits are shared through PostgreSQL, so no sticky sessions.
- 🧩 **Extend it your way.** Hooks, routes, cron jobs and a query builder in JavaScript files,
  no build step; or a Rust library API.
- 🗂️ **Many projects, one server.** Host several isolated apps from one installation, each in a
  database of its own.
- 🚀 **Fast where it matters.** Written in Rust with tokio and axum; filters compile to
  parameterized SQL that uses your indexes ([benchmarks](bench/README.md)).
- 📦 **One static binary.** An 8 MB download with the dashboard built in. Install it with
  Docker, `.deb`/`.rpm` packages or a plain binary, and finish with a setup wizard.

## Features

<table>
<tr>
<td width="50%" valign="top">

**Data**
- Base, auth and view collections, with 14 field types
- A filter language for queries and access rules: relation paths, `@request.*`, `@collection.*`
- `expand`, back-relations, `fields`, sorting and paging
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
- Backups (on a schedule, to S3) and one-click restores
- JavaScript hooks, cron jobs, a query builder, and a Rust library API
- Migrations in JavaScript, each in a transaction, with automigrate

</td>
</tr>
</table>

> [!NOTE]
> **slonbase is young (0.8, before 1.0).** Expect rough edges, and back up your data. Bug reports
> are very welcome.

## Coming from PocketBase?

slonbase is inspired by [PocketBase](https://pocketbase.io), and it's an independent project,
written from scratch in Rust, with its own design. Its API is compatible with PocketBase's, so you
can bring an existing app along:

- the PocketBase JavaScript and Dart SDKs work against slonbase as they are;
- most `pb_hooks` and `pb_migrations` files run unchanged;
- `slonbase import-pocketbase` brings over collections, records, files and settings, with users'
  password hashes, so nobody has to reset a password.

A test suite written with the official SDK runs against both servers in CI to keep it that way
([compat/](compat/)). The [migration guide](docs/migrating-from-pocketbase.md) lists the
differences.

PocketBase is the better choice when you want zero moving parts (one file, SQLite inside), when
one small app on one small machine is the whole plan, or when you extend it in Go.

## Quick start

**In 60 seconds, with Docker.** This starts slonbase and a PostgreSQL 17 server for it:

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

**Without Docker**, PostgreSQL isn't bundled: slonbase connects to any PostgreSQL 14+ server, on
the same machine, elsewhere, or a managed service, and the setup link asks for it.

**Debian / Ubuntu:** from the apt repository, so `apt upgrade` keeps it up to date:

```sh
curl -fsSL https://ayeshrodz.github.io/slonbase/slonbase.gpg | sudo tee /usr/share/keyrings/slonbase.gpg > /dev/null
curl -fsSL https://ayeshrodz.github.io/slonbase/slonbase.list | sudo tee /etc/apt/sources.list.d/slonbase.list
sudo apt update && sudo apt install slonbase
sudo journalctl -u slonbase | grep "setup?token"
```

**Fedora / RHEL:** `sudo curl -fsSL -o /etc/yum.repos.d/slonbase.repo https://ayeshrodz.github.io/slonbase/slonbase.repo && sudo dnf install slonbase`.
The `.deb` and `.rpm` files are also on the [releases](https://github.com/ayeshrodz/slonbase/releases) page.

**Binary:** extract the `.tar.gz` from the releases and run `./slonbase serve`.

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
| **API rules** | **API preview** |
| <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/rules-dark.png"><img alt="API rules" src="docs/assets/screenshots/rules-light.png"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/api-dark.png"><img alt="API preview" src="docs/assets/screenshots/api-light.png"></picture> |

<picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/diagram-dark.png"><img alt="The schema diagram: every collection with its fields, and its relations" src="docs/assets/screenshots/diagram-light.png"></picture>
<p align="center"><sub>The schema diagram, drawn from the collections: hover one to follow its relations.</sub></p>

The dashboard follows your system's light or dark theme, works on phones, and ⌘K / Ctrl K jumps
anywhere. It only uses the public API, so anything it does, you can script too.

## Documentation

The guides are also on the documentation site, with search:
**[ayeshrodz.github.io/slonbase/docs](https://ayeshrodz.github.io/slonbase/docs/)**.

- [Installation](docs/installation.md): Docker, packages, binaries, first-run setup, upgrades
- [Configuration](docs/configuration.md): server options and application settings
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

Compared with PocketBase, on the same machine, the same data and the same HTTP workloads, with
default settings ([method and full results](bench/README.md)):

| Scenario | slonbase req/s | PocketBase req/s | p99 ms (slonbase / PocketBase) |
|---|--:|--:|--:|
| List 30 records | 1,557 | 1,043 | 36.5 / 101.8 |
| List 30, filtered | 1,175 | 73 | 47.7 / 623.3 |
| List 30, expand a relation | 973 | 701 | 50.5 / 161.5 |
| Create a record | 1,728 (2,792 with async commit) | 2,532 | 29.8 / 66.0 |

An embedded SQLite answers a single client's lookups faster, since it runs in the server's
process. Writes are slower with PostgreSQL's default durability, and a little faster when
PostgreSQL makes the same trade-off as SQLite's default (`synchronous_commit = off`; see
[why](docs/deployment.md#postgresql)).

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

[MIT](LICENSE). The dashboard vendors Preact (MIT) and htm (Apache-2.0); their licenses are in
[ui/app/vendor](ui/app/vendor). It ships the Ubuntu Sans and Ubuntu Sans Mono fonts (Ubuntu
Font Licence 1.0); the licence is in [ui/app/fonts](ui/app/fonts).
slonbase is an independent project inspired by PocketBase (also MIT). It is not affiliated
with or endorsed by PocketBase or its author, and it wouldn't exist without their work.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-dark.svg">
    <img alt="slonbase" src="docs/assets/banner-light.svg" width="640">
  </picture>
</p>

<p align="center">
  <strong>PocketBase's API, on PostgreSQL.</strong><br>
  An open-source backend in one Rust binary: database, auth, realtime, files and an admin
  dashboard. Your PocketBase apps, SDKs, hooks and migrations keep working.
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
  <a href="#quick-start">Quick start</a> ·
  <a href="docs/installation.md">Install</a> ·
  <a href="docs/api.md">API</a> ·
  <a href="docs/extending.md">Hooks</a> ·
  <a href="docs/migrating-from-pocketbase.md">From PocketBase</a> ·
  <a href="bench/README.md">Benchmarks</a>
</p>

<p align="center">
  <img alt="The official PocketBase JavaScript SDK creating and updating a task, and the slonbase dashboard's grid updating live" src="docs/assets/demo.gif" width="900">
</p>

<p align="center">
  <sub>The unmodified <code>pocketbase</code> npm package talking to slonbase. The grid is live: realtime events arrive as the code runs.</sub>
</p>

<p align="center">
  <b>Tested against PocketBase on every commit:</b> the same test suite, written with the official
  JavaScript SDK, runs against a real PocketBase 0.40 and against slonbase, and must pass on both
  (<a href="compat/">compat/</a>).
</p>

## Why slonbase

[PocketBase](https://pocketbase.io) made backends delightful: define collections in a dashboard,
and get a REST API, auth, realtime and files right away. slonbase keeps that experience and its
API, and puts it on **PostgreSQL**:

- 🧩 **Drop-in for PocketBase apps.** The official SDKs work unchanged, and so do most `pb_hooks`
  and `pb_migrations`.
  `slonbase import-pocketbase` brings over collections, records, files and settings. It even
  brings users' password hashes, so nobody has to reset a password.
- 🐘 **Your data in PostgreSQL.** Each collection is a real table that you can query with SQL,
  replicate, back up with `pg_dump`, or run on a managed service.
- 🗂️ **Many projects, one server.** Host several isolated apps from one installation. Each is
  served under `/p/<name>/` and kept in a database of its own.
- ↔️ **Scales out.** Run several instances on one database behind a load balancer. Realtime
  events, schema changes and settings reach every instance through `LISTEN/NOTIFY`.
- ⚡ **Fast where it matters.** It's built in Rust with tokio and axum, its filters compile to
  parameterized SQL, and PostgreSQL's planner uses your indexes. In
  [the benchmarks](bench/README.md), lists are about 1.5× faster than PocketBase, filtered
  lists up to **16×**, and p99 latency is about a third of PocketBase's.
- 📦 **One static binary.** It's an 8 MB download, with the dashboard built in. Install it with
  Docker, `.deb`/`.rpm` packages, or a plain binary, then finish with a setup wizard.

## Features

<table>
<tr>
<td width="50%" valign="top">

**Data**
- Base, auth and view collections, with 14 field types
- PocketBase's filter syntax and API rules: relation paths, `@request.*`, `@collection.*`
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
- File fields on disk or any S3-compatible bucket
- Thumbnails, protected files, byte ranges

**Operations**
- An admin dashboard with live grids, a schema editor, logs and the SQL console
- Request logs, rate limits, superuser IP restrictions
- Backups (on a schedule, to S3) and one-click restores
- JavaScript hooks (`*.pb.js`), cron jobs, and a Rust library API
- JavaScript migrations (`pb_migrations`), each in a transaction, with automigrate

</td>
</tr>
</table>

> [!NOTE]
> **slonbase is young (0.4).** Expect rough edges, and back up your data. Compatibility reports
> are very welcome: if your PocketBase app doesn't work on slonbase, that's a bug.

## When to use PocketBase instead

slonbase is not a replacement for every PocketBase install. PocketBase is the better choice when:

- **You want zero moving parts.** PocketBase is one file with SQLite inside. slonbase needs a
  PostgreSQL server next to it (the Compose file below starts one).
- **One small app on one small machine is the whole plan.** SQLite in the same process answers
  a single client's lookups faster than any network database.
- **You extend it in Go** (`main.go`), or rely on a hook slonbase doesn't have yet (the
  collection request hooks, `onBatchRequest`, `$dbx`). See
  [the differences](docs/migrating-from-pocketbase.md).

slonbase is for when you want the PocketBase experience but your data belongs in PostgreSQL:
you already run it, you want SQL access, replicas, `pg_dump` or a managed service, or you need
more than one server instance.

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

Then use any PocketBase SDK, unchanged:

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

The dashboard follows your system's light or dark theme, works on phones, and press ⌘K / Ctrl K
to jump anywhere. It only uses the public API, so anything it does, you can script too.

## Documentation

- [Installation](docs/installation.md): Docker, packages, binaries, first-run setup, upgrades
- [Configuration](docs/configuration.md): server options and application settings
- [Hosting platforms](docs/platforms.md): Render, Railway, Fly.io, Coolify and Dokploy
- [Running in production](docs/deployment.md): reverse proxy, PostgreSQL, several instances,
  backups, monitoring, security
- [API](docs/api.md): endpoints, collections, filters and rules, files, realtime
- [Extending](docs/extending.md): JavaScript hooks and the Rust library
- [Migrations](docs/migrations.md): collections in code, PocketBase's `pb_migrations` included
- [Projects](docs/projects.md): several isolated installations, one database each, on one server
- [Migrating from PocketBase](docs/migrating-from-pocketbase.md): the importer and the differences
- [Benchmarks](bench/README.md): slonbase and PocketBase on the same workloads
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

Same machine, same data, the same HTTP API and the official SDK, with default settings
([method and full results](bench/README.md)):

| Scenario | slonbase req/s | PocketBase req/s | p99 ms (slonbase / PocketBase) |
|---|--:|--:|--:|
| List 30 records | 1,557 | 1,043 | 36.5 / 101.8 |
| List 30, filtered | 1,175 | 73 | 47.7 / 623.3 |
| List 30, expand a relation | 973 | 701 | 50.5 / 161.5 |
| Create a record | 1,728 (2,792 with async commit) | 2,532 | 29.8 / 66.0 |

PocketBase answers a single client's lookups faster, because SQLite runs in its process. Writes
are slower with PostgreSQL's default durability, and a little faster than PocketBase's when
PostgreSQL makes the same trade-off as SQLite's default (`synchronous_commit = off`; see
[why](docs/deployment.md#postgresql)).

## Contributing

Bug reports, compatibility reports and pull requests are welcome; see
[CONTRIBUTING.md](CONTRIBUTING.md). The quickest checks:

```sh
cargo test                                                    # unit tests
SLONBASE_TEST_DATABASE_URL='postgres://postgres:postgres@127.0.0.1:5432/postgres?sslmode=disable' cargo test   # everything
(cd compat && npm ci) && DATABASE_URL='postgres://...' compat/run.sh slonbase                # the SDK suite
```

Please report security problems [privately](SECURITY.md).

## License

[MIT](LICENSE). The dashboard vendors Preact (MIT) and htm (Apache-2.0); their licenses are in
[ui/app/vendor](ui/app/vendor).
slonbase is an independent project inspired by PocketBase (also MIT). It is not affiliated
with or endorsed by PocketBase or its author, and it wouldn't exist without their work.

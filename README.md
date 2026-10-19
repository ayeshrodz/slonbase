<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-dark.svg">
    <img alt="slonbase" src="docs/assets/banner-light.svg" width="640">
  </picture>
</p>

<p align="center">
  <strong>An open-source backend in one binary: database, auth, realtime, files and an admin dashboard.</strong><br>
  The PocketBase API you know, built in Rust on top of your own PostgreSQL.
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
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/records-dark.png">
    <img alt="The slonbase dashboard: a collection's records in a live grid" src="docs/assets/screenshots/records-light.png" width="900">
  </picture>
</p>

## Why slonbase

[PocketBase](https://pocketbase.io) made backends delightful: define collections in a dashboard,
and get a REST API, auth, realtime and files right away. slonbase keeps that experience and its
API, and puts it on **PostgreSQL**:

- 🧩 **Drop-in for PocketBase apps.** The official SDKs work unchanged, and so do most `pb_hooks`.
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
  [the benchmarks](bench/README.md), filtered lists are up to **14× faster** than PocketBase,
  and p99 latency is about half of PocketBase's.
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

</td>
</tr>
</table>

> [!NOTE]
> **0.1 is the first release.** The API is tested against PocketBase 0.40 with its official
> JavaScript SDK, on every commit ([compat/](compat/)). Expect rough edges, and back up your data.

## Quick start

PostgreSQL isn't bundled. slonbase connects to any PostgreSQL 14+ server, on the same machine,
elsewhere, or a managed service. The Compose file below starts one for you.

**Docker Compose:**

```sh
git clone https://github.com/ayeshrodz/slonbase && cd slonbase
cp .env.example .env              # set POSTGRES_PASSWORD
docker compose up -d
docker compose logs slonbase | grep "setup?token"
```

**Debian / Ubuntu / Fedora / RHEL:** download the package for your architecture from the
[releases](https://github.com/ayeshrodz/slonbase/releases), then:

```sh
sudo apt install ./slonbase_0.1.0_amd64.deb        # or: sudo dnf install ./slonbase-0.1.0-1.x86_64.rpm
sudo journalctl -u slonbase | grep "setup?token"
```

**Binary:** extract the `.tar.gz` from the releases and run `./slonbase serve`.

Open the printed link to connect PostgreSQL and create your first superuser. The dashboard is at
`/_/` (e.g. `http://localhost:8090/_/`) and the API at `/api/`.

```js
import PocketBase from "pocketbase";

const pb = new PocketBase("http://localhost:8090");
await pb.collection("users").authWithPassword("ann@example.com", "secret");
const posts = await pb.collection("posts").getList(1, 20, { filter: 'published = true', expand: "author" });
```

## A look around

| Overview | Schema |
|---|---|
| <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/overview-dark.png"><img alt="Overview" src="docs/assets/screenshots/overview-light.png"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/schema-dark.png"><img alt="Schema editor" src="docs/assets/screenshots/schema-light.png"></picture> |
| **API rules** | **API preview** |
| <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/rules-dark.png"><img alt="API rules" src="docs/assets/screenshots/rules-light.png"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/api-dark.png"><img alt="API preview" src="docs/assets/screenshots/api-light.png"></picture> |

<p align="center"><b>Logs</b><br>
  <picture><source media="(prefers-color-scheme: dark)" srcset="docs/assets/screenshots/logs-dark.png"><img alt="Logs" src="docs/assets/screenshots/logs-light.png" width="640"></picture>
</p>

The dashboard follows your system's light or dark theme, works on phones, and press ⌘K / Ctrl K
to jump anywhere. It only uses the public API, so anything it does, you can script too.

## Documentation

- [Installation](docs/installation.md): Docker, packages, binaries, first-run setup, upgrades
- [Configuration](docs/configuration.md): server options and application settings
- [Running in production](docs/deployment.md): reverse proxy, PostgreSQL, several instances,
  backups, monitoring, security
- [API](docs/api.md): endpoints, collections, filters and rules, files, realtime
- [Extending](docs/extending.md): JavaScript hooks and the Rust library
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

`slonbase --help` lists every option.

## Performance

Same machine, same data, the same HTTP API and the official SDK, with default settings
([method and full results](bench/README.md)):

| Scenario | slonbase req/s | PocketBase req/s | p99 ms (slonbase / PocketBase) |
|---|--:|--:|--:|
| List 30 records | 1,088 | 928 | 53.8 / 123.4 |
| List 30, filtered | 836 | 60 | 75.1 / 711.4 |
| List 30, expand a relation | 709 | 654 | 70.5 / 176.3 |
| Create a record | 1,417 (2,220 with async commit) | 2,350 | 36.1 / 72.8 |

PocketBase answers a single client's lookups faster, because SQLite runs in its process. Writes
match it when PostgreSQL trades the same durability that SQLite's default does
(`synchronous_commit = off`; see [why](docs/deployment.md#postgresql)).

## Contributing

Bug reports, compatibility reports and pull requests are welcome; see
[CONTRIBUTING.md](CONTRIBUTING.md). The quickest checks:

```sh
cargo test                                                    # unit tests
SLONBASE_TEST_DATABASE_URL='postgres://postgres:postgres@127.0.0.1:5432/postgres' cargo test   # everything
(cd compat && npm ci) && DATABASE_URL='postgres://...' compat/run.sh slonbase                # the SDK suite
```

Please report security problems [privately](SECURITY.md).

## License

[MIT](LICENSE). The dashboard vendors Preact (MIT) and htm (Apache-2.0); their licenses are in
[ui/app/vendor](ui/app/vendor).
slonbase is an independent project inspired by PocketBase (also MIT) and is not
affiliated with it.

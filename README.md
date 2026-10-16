# slonbase

A lightweight, high-performance backend in a single Rust binary, powered by **PostgreSQL**.
Inspired by [PocketBase](https://pocketbase.io): define collections, and get a REST API, auth,
realtime, file storage and an admin dashboard, on top of your own PostgreSQL server instead of
SQLite.

- **PocketBase-compatible API.** The official PocketBase SDKs work, and so do most `pb_hooks`.
  `slonbase import-pocketbase` moves an existing PocketBase app over, users' passwords included.
- **PostgreSQL underneath.** Collections are real tables you can query, replicate and back up
  with the usual tools. Several instances can share one database, and realtime events reach all
  of them.
- **Everything in one static binary** (an 8 MB download): the records API with filters and API
  rules, auth (password, OAuth2, OTP, MFA), realtime subscriptions, files on disk or in S3 with
  thumbnails, batch requests, rate limits, request logs, backups, cron jobs, JavaScript and Rust
  hooks, and the dashboard.
- **Easy to run:** Docker Compose, `.deb`/`.rpm` packages with a hardened systemd service, or a
  static binary, with a first-run setup wizard.

PostgreSQL is **not bundled**: slonbase connects to any PostgreSQL 14+ server, whether it runs on
the same machine, another machine, or a managed service.

> **Status:** 0.1, the first release. The API is tested against PocketBase 0.40 with its
> JavaScript SDK (see [compat/](compat/)). Expect rough edges, and back up your data.

## Quick start

**Docker Compose** (slonbase and PostgreSQL):

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

## Documentation

- [Installation](docs/installation.md): Docker, packages, binaries, first-run setup, upgrades
- [Configuration](docs/configuration.md): server options and application settings
- [Running in production](docs/deployment.md): reverse proxy, PostgreSQL, several instances,
  backups, monitoring, security
- [API](docs/api.md): endpoints, collections, filters and rules, files, realtime
- [Extending](docs/extending.md): JavaScript hooks and the Rust library
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

## Development

```sh
cargo test                     # unit tests
# integration tests need a PostgreSQL role that can create databases:
SLONBASE_TEST_DATABASE_URL='postgres://postgres:postgres@127.0.0.1:5432/postgres?sslmode=disable' cargo test
# S3 tests also need an S3-compatible server (path-style), e.g. MinIO:
SLONBASE_TEST_S3_ENDPOINT=http://127.0.0.1:9000 SLONBASE_TEST_S3_ACCESS_KEY=... SLONBASE_TEST_S3_SECRET=... cargo test
```

The compatibility suite in [`compat/`](compat/) drives the official PocketBase JavaScript SDK
against a running server: CI runs it against slonbase and against PocketBase itself, so its
expectations are PocketBase's actual behavior.

```sh
(cd compat && npm ci)
cargo build && DATABASE_URL='postgres://...an empty database...' compat/run.sh slonbase
POCKETBASE_BIN=/path/to/pocketbase compat/run.sh pocketbase
```

[bench/](bench/) compares slonbase and PocketBase on the same workloads; see
[bench/README.md](bench/README.md) for the results.

Packages are built with [nfpm](https://nfpm.goreleaser.com):

```sh
cargo build --release --locked --target x86_64-unknown-linux-musl
scripts/package.sh x86_64-unknown-linux-musl amd64 0.1.0     # → dist/
```

Pushing a `v*` tag publishes a GitHub release with `.deb`, `.rpm` and `.tar.gz` files for amd64
and arm64, plus a multi-arch image at `ghcr.io/ayeshrodz/slonbase`.

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to contribute.

## License

[MIT](LICENSE). The dashboard vendors Preact (MIT) and htm (Apache-2.0); their licenses are in
[ui/app/vendor](ui/app/vendor).
slonbase is an independent project inspired by PocketBase (also MIT) and is not
affiliated with it.

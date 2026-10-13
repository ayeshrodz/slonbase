# slonbase

A lightweight, high-performance backend in a single Rust binary, powered by **PostgreSQL**.
Inspired by [PocketBase](https://pocketbase.io): define collections, get a REST API, auth and an
admin dashboard, but on top of your own PostgreSQL server instead of SQLite.

> **Status: early development (phase 0).** The server, first-run setup, packaging and CI are in place.
> Collections, the REST API, auth and the admin dashboard are next. See the [roadmap](#roadmap).

PostgreSQL is **not bundled**. slonbase connects to any PostgreSQL 14+ server, on the same machine,
another machine or a managed service, and sets everything up from a first-run wizard.

## Quick start

### Docker Compose (slonbase + PostgreSQL)

```sh
cp .env.example .env     # set POSTGRES_PASSWORD
docker compose up -d
docker compose logs slonbase | grep "setup?token"
```

Open the printed link to create your first superuser.

### Debian / Ubuntu / Fedora packages

Download the `.deb` or `.rpm` for your architecture from the
[releases](https://github.com/ayeshrodz/slonbase/releases) page, then:

```sh
sudo apt install ./slonbase_<version>_amd64.deb      # or: sudo dnf install ./slonbase-<version>-1.x86_64.rpm
sudo journalctl -u slonbase | grep "setup?token"     # the one-time setup link
```

The package installs a hardened systemd service (`slonbase.service`) running as a dedicated
`slonbase` user, with its config in `/etc/slonbase/config.toml`. An apt repository is planned
so updates arrive through `apt upgrade`.

### Plain binary

```sh
./slonbase serve          # starts in setup mode and prints the setup link
```

## First-run setup

On first start, slonbase has no database configured, so it starts in **setup mode** and prints a
one-time link (the token in it is required, so nobody else reaching the port can take over the
install). The wizard:

1. asks for the PostgreSQL connection and tests it (version, permissions, existing installs),
2. creates the system tables in their own `_slonbase` schema (the database can be shared),
3. creates the first superuser,
4. saves the connection to the config file (`0600` permissions) and switches to normal mode
   without a restart.

When the database URL comes from the environment (as in Docker Compose), the wizard skips step 1.
Pointing a new server at a database that already has a slonbase installation requires signing in
with one of its superusers.

Once set up, slonbase never returns to setup mode: if the database goes down, requests fail with
`503` while it reconnects.

Headless servers can skip the wizard:

```sh
SLONBASE_SUPERUSER_PASSWORD='...' slonbase setup \
    --database-url 'postgres://user:pass@host:5432/db' --email you@example.com
```

## Configuration

Settings come from (highest priority first) command-line flags, environment variables, the config
file, then defaults.

| Flag | Environment variable | Config file | Default |
|---|---|---|---|
| `--config` | `SLONBASE_CONFIG` | | `slonbase.toml` |
| `--bind` | `SLONBASE_BIND` | `server.bind` | `127.0.0.1:8090` |
| `--database-url` | `SLONBASE_DATABASE_URL` | `database.url` | none (setup mode) |
| `--database-max-connections` | `SLONBASE_DATABASE_MAX_CONNECTIONS` | `database.max_connections` | `16` |
| `--database-ca-file` | `SLONBASE_DATABASE_CA_FILE` | `database.ca_file` | none |
| `serve --setup-token` | `SLONBASE_SETUP_TOKEN` | | random |

Database URLs use the standard libpq format, e.g.
`postgres://user:pass@host:5432/db?sslmode=require`. TLS uses the Mozilla root certificates, so
add `--database-ca-file` for providers with their own CA (such as Amazon RDS).

Logging is controlled by `RUST_LOG` (default `info`).

## Commands

| Command | Description |
|---|---|
| `slonbase serve` | Start the server (default). |
| `slonbase setup` | Set up without the web wizard. |
| `slonbase health` | Exit 0 if the local server is healthy (used by the Docker health check). |

HTTP endpoints so far: `GET /api/health`, the setup API under `/api/setup`, and the wizard at `/_/setup`.

## Development

```sh
cargo test                     # unit tests
# integration tests need a PostgreSQL role that can create databases:
SLONBASE_TEST_DATABASE_URL='postgres://postgres:postgres@127.0.0.1:5432/postgres?sslmode=disable' cargo test
```

Packages are built with [nfpm](https://nfpm.goreleaser.com):

```sh
cargo build --release --locked --target x86_64-unknown-linux-musl
scripts/package.sh x86_64-unknown-linux-musl amd64 0.1.0     # → dist/
```

Pushing a `v*` tag publishes a GitHub release with `.deb`, `.rpm` and `.tar.gz` files for amd64
and arm64, plus a multi-arch image at `ghcr.io/ayeshrodz/slonbase`.

The design and the reasoning behind it are in [docs/design.md](docs/design.md).

## Roadmap

0. **Foundation** (done): server, config, first-run setup, system migrations, packaging, CI.
1. **Collections**: schema management, CRUD, filtering/sorting/pagination, API rules.
2. **Auth**: superuser and user auth, then OAuth2, OTP and MFA.
3. **Realtime, files and relation expansion.**
4. **Admin dashboard**, mail, logs, backups, settings, batch requests.
5. **Extensibility**: use as a Rust library, and JS hooks.

## License

[MIT](LICENSE). slonbase is an independent project inspired by PocketBase (also MIT) and is not
affiliated with it.

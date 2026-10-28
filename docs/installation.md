# Installation

slonbase is a single binary. It needs a **PostgreSQL 14 or newer** server, which it doesn't
bundle: use one on the same machine, another machine, or a managed service (Amazon RDS, Google
Cloud SQL, Azure, Supabase, Neon, ...). Everything slonbase creates lives in two schemas,
`_slonbase` (system tables) and `slonbase` (your collections), so the database can be shared with
other applications.

Release builds are static Linux binaries for x86_64 (amd64) and aarch64 (arm64). Other platforms
can [build from source](#from-source).

- [Docker Compose](#docker-compose)
- [Docker](#docker)
- [Hosting platforms](platforms.md): Render, Railway, Fly.io, Coolify, Dokploy
- [Debian, Ubuntu, Fedora, RHEL packages](#debian-ubuntu-fedora-rhel-packages)
- [Plain binary](#plain-binary)
- [From source](#from-source)
- [First-run setup](#first-run-setup)
- [Upgrading](#upgrading)
- [Uninstalling](#uninstalling)

## Docker Compose

[`compose.yaml`](../compose.yaml) runs slonbase and PostgreSQL 17 together:

```sh
mkdir slonbase && cd slonbase
curl -fsSLO https://raw.githubusercontent.com/ayeshrodz/slonbase/main/compose.yaml
echo "POSTGRES_PASSWORD=$(openssl rand -hex 24)" > .env   # URL-safe, see .env.example for the rest
docker compose up -d
docker compose logs slonbase | grep "setup?token"
```

Open the printed link (the token in it is required) to create the first superuser. Data lives in
two volumes: `pgdata` (the database) and `slonbase_data` (`/data`: files, backups, hooks and the
config file).

`SLONBASE_VERSION` in `.env` pins a release (`1.0.1`); the default is `latest`. When the image
can't be pulled, or with `docker compose up --build`, Compose builds it from a checkout of the
repository (run it there).

## Docker

The image `ghcr.io/ayeshrodz/slonbase` (amd64 and arm64) contains only the binary. It runs as an
unprivileged user (65532), listens on `0.0.0.0:8090`, keeps everything under the `/data` volume,
and has a health check (`slonbase health`).

```sh
docker run -d --name slonbase -p 8090:8090 -v slonbase_data:/data \
  -e SLONBASE_DATABASE_URL='postgres://user:pass@db.example.com:5432/app?sslmode=require' \
  ghcr.io/ayeshrodz/slonbase:1.0.1
docker logs slonbase 2>&1 | grep "setup?token"
```

Without `SLONBASE_DATABASE_URL` the setup wizard asks for the connection and saves it in
`/data/slonbase.toml`. JavaScript hooks go in `/data/hooks`.

## Debian, Ubuntu, Fedora, RHEL packages

The packages are in signed apt and dnf repositories at
<https://ayeshrodz.github.io/slonbase/>, for amd64 and arm64. Installing from them means
`apt upgrade` and `dnf upgrade` bring new versions.

**Debian, Ubuntu:**

```sh
curl -fsSL https://ayeshrodz.github.io/slonbase/slonbase.gpg | sudo tee /usr/share/keyrings/slonbase.gpg > /dev/null
curl -fsSL https://ayeshrodz.github.io/slonbase/slonbase.list | sudo tee /etc/apt/sources.list.d/slonbase.list
sudo apt update && sudo apt install slonbase
sudo journalctl -u slonbase | grep "setup?token"     # the one-time setup link
```

**Fedora, RHEL, Rocky, Alma:**

```sh
sudo curl -fsSL -o /etc/yum.repos.d/slonbase.repo https://ayeshrodz.github.io/slonbase/slonbase.repo
sudo dnf install slonbase                            # accept the signing key when asked
sudo journalctl -u slonbase | grep "setup?token"
```

The signing key's fingerprint is shown on the repository page. To install a package file
instead, download the `.deb` or `.rpm` for your architecture from the
[releases page](https://github.com/ayeshrodz/slonbase/releases) and check it against
`SHA256SUMS`:

```sh
sha256sum --ignore-missing -c SHA256SUMS
sudo apt install ./slonbase_1.0.1_amd64.deb          # Debian, Ubuntu
sudo dnf install ./slonbase-1.0.1-1.x86_64.rpm       # Fedora, RHEL, Rocky, Alma
sudo journalctl -u slonbase | grep "setup?token"     # the one-time setup link
```

The packages recommend (but don't require) the distribution's PostgreSQL server. They install:

| Path | |
|---|---|
| `/usr/bin/slonbase` | the binary |
| `/usr/lib/systemd/system/slonbase.service` | a hardened systemd service, enabled and started on install |
| `/etc/default/slonbase` | the service's environment (listen address, log level) |
| `/etc/slonbase/config.toml` | the config file, written by the setup wizard (`0600`) |
| `/var/lib/slonbase/` | `storage/` (files), `backups/`, `hooks/` |

The service runs as the `slonbase` system user and listens on `127.0.0.1:8090`. To reach the
setup wizard from another machine, use an SSH tunnel (`ssh -L 8090:127.0.0.1:8090 server`), or
set `SLONBASE_BIND=0.0.0.0:8090` in `/etc/default/slonbase` and
`sudo systemctl restart slonbase`. For production, put a reverse proxy in front instead (see
[deployment](deployment.md#reverse-proxy)).

If PostgreSQL runs on the same machine, create a role and a database for slonbase:

```sh
sudo -u postgres psql -c "CREATE ROLE slonbase LOGIN PASSWORD 'change-me'" \
                      -c "CREATE DATABASE slonbase OWNER slonbase"
```

and enter `postgres://slonbase:change-me@127.0.0.1:5432/slonbase` in the wizard.

## Plain binary

```sh
curl -LO https://github.com/ayeshrodz/slonbase/releases/download/v1.0.1/slonbase-1.0.1-linux-amd64.tar.gz
tar xzf slonbase-1.0.1-linux-amd64.tar.gz
./slonbase serve      # starts in setup mode and prints the setup link
```

By default it listens on `127.0.0.1:8090` and keeps its config (`slonbase.toml`) and data
(`slonbase_data/`, `slonbase_hooks/`) in the current directory; see
[configuration](configuration.md).

## From source

Rust 1.87 or newer:

```sh
git clone https://github.com/ayeshrodz/slonbase && cd slonbase
cargo build --release --locked
./target/release/slonbase serve
```

`--no-default-features` leaves out the JavaScript engine (no `*.pb.js` hooks, a smaller binary).
`docker build .` builds the image from source.

## First-run setup

On first start slonbase has no database configured, so it starts in **setup mode** and prints a
one-time link. The token in it is required, so nobody else who can reach the port can take over
the installation. The wizard:

1. asks for the PostgreSQL connection and tests it (version, permissions, existing installs);
2. creates the system tables in the `_slonbase` schema;
3. creates the first superuser;
4. saves the connection in the config file (`0600`) and switches to normal mode without a
   restart.

When the database URL comes from the environment or a flag (as with Docker Compose), the wizard
skips step 1. Pointing a new server at a database that already has a slonbase installation
requires signing in as one of its superusers.

The database role needs to create schemas and tables in its database: the owner of the database
is enough; it doesn't need to be a PostgreSQL superuser.

Once set up, slonbase never returns to setup mode. If the database goes down, requests fail with
`503` while it reconnects.

**Headless setup** skips the wizard:

```sh
SLONBASE_SUPERUSER_PASSWORD='...' slonbase setup \
    --database-url 'postgres://user:pass@host:5432/db' --email you@example.com
```

(for the packages, run it as the service user:
`sudo -u slonbase env SLONBASE_SUPERUSER_PASSWORD=... slonbase setup --config /etc/slonbase/config.toml ...`).

**Lost superuser access?** On the server, `slonbase superuser upsert you@example.com 'new password'`
creates or resets a superuser, and `slonbase superuser otp you@example.com` prints a one-time
password for signing in when MFA is on and emails can't be sent. Pass the same `--config` (or
environment) the server uses.

## Upgrading

Back up first (a slonbase backup, a database dump, or both; see
[deployment](deployment.md#backups)). Then:

- **Compose:** `docker compose pull && docker compose up -d` (or set `SLONBASE_VERSION`).
- **Docker:** run the new tag with the same volume and environment.
- **Packages:** `sudo apt update && sudo apt upgrade` or `sudo dnf upgrade` with the
  [repositories](#debian-ubuntu-fedora-rhel-packages) (an installation from a package file
  can add them at any time), or install the new `.deb`/`.rpm` file. The service restarts with
  the new version, and `/etc/default/slonbase` and `/etc/slonbase/config.toml` are kept.
- **Binary:** replace the file and restart.

A new version applies its system migrations on start, in one transaction under an advisory lock,
so several instances starting at once apply them once, and a failed migration changes nothing.
Downgrading after a migration isn't supported: restore the backup instead. Every change is tested
for upgrades in place from the first release (0.1.0) and from the previous one: CI seeds the
old release with data, starts the new build on it, and checks the data, files, settings, old
passwords and tokens, rules, realtime and backups (`compat/upgrade/run.sh`).

## Uninstalling

- **Packages:** `sudo apt remove slonbase` / `sudo dnf remove slonbase`. The config and
  `/var/lib/slonbase` are kept (`apt purge` removes them). To remove the repository too:
  `sudo rm /etc/apt/sources.list.d/slonbase.list /usr/share/keyrings/slonbase.gpg` or
  `sudo rm /etc/yum.repos.d/slonbase.repo`.
- **Docker:** remove the container and, if you no longer need it, the `/data` volume.

slonbase never drops the database. To remove its data from PostgreSQL:
`DROP SCHEMA slonbase CASCADE; DROP SCHEMA _slonbase CASCADE;`.

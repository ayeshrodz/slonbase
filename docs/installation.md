# Installation

slonbase is a single binary. It needs a **PostgreSQL 14 or newer** server, which it doesn't
bundle: use one on the same machine, another machine, or a managed service (Amazon RDS, Google
Cloud SQL, Azure, Supabase, Neon, ...). Everything slonbase creates lives in two schemas,
`_slonbase` (system tables) and `slonbase` (your collections), so the database can be shared with
other applications.

So an installation has two parts: **a database** (a PostgreSQL server, a role slonbase logs in
as, and an empty database owned by it) and **slonbase** (the program, which creates its tables
in that database and a first superuser who signs in to the dashboard). The quick install below
does both in one command; the step-by-step section does the same by hand.

Release builds are static Linux binaries for x86_64 (amd64) and aarch64 (arm64), with
`.deb` and `.rpm` packages and a Docker image. From 2.7.0 there are also **experimental** archives
for [macOS](#macos) (Apple silicon) and [Windows](#windows) (x64): a `.tar.gz` and a `.zip`, not
signed, that you check against the release's checksums. Other platforms can
[build from source](#from-source).

- [Quick install on Linux](#quick-install-on-linux): one command, on Debian, Ubuntu, Fedora, RHEL
- [Step by step on Linux](#step-by-step-on-linux): the same, by hand
- [Docker Compose](#docker-compose) and [Docker](#docker)
- [Hosting platforms](platforms.md): Render, Railway, Fly.io, Coolify, Dokploy
- [Plain binary](#plain-binary), [macOS](#macos), [Windows](#windows) and [from source](#from-source)
- [First-run setup](#first-run-setup)
- [Check that it works](#check-that-it-works) and [troubleshooting](#troubleshooting)
- [Upgrading](#upgrading) and [uninstalling](#uninstalling)

## Quick install on Linux

On a fresh Debian, Ubuntu, Fedora, RHEL, Rocky or Alma server (amd64 or arm64, with systemd):

```sh
curl -fsSL https://slonbase.com/install.sh | sudo sh
```

(Prefer to read it first? `curl -fsSLO https://slonbase.com/install.sh`, look at it, then
`sudo sh install.sh`. It is about 250 lines of shell.)

If it stops partway (a network failure, a typo in the email), run it again. The role it created
and its generated password are kept in `/etc/slonbase/install.env` (readable by root only)
until setup succeeds, and the rerun continues from there.

It asks for the email of the first superuser, then:

1. adds the signed apt or dnf repository and installs the `slonbase` package, so that
   `apt upgrade` and `dnf upgrade` keep it current (first, so that a failed download changes
   nothing in PostgreSQL);
2. installs PostgreSQL from your distribution, if the machine has none, and checks it is 14 or
   newer (on RHEL 8 and 9 it enables the `postgresql:16` module);
3. creates a PostgreSQL role `slonbase` and an empty database `slonbase` owned by it, with a
   generated password (it never changes an existing role's password or touches an existing
   database; if the names are taken it stops and tells you how to choose others). On Red Hat's
   family it adds password logins on localhost for that role and database only, ahead of the
   default `ident` lines, and keeps the original file as `pg_hba.conf.slonbase.bak`;
4. creates the first superuser with a generated password, saves the database connection in
   `/etc/slonbase/config.toml` (readable only by the `slonbase` user) and starts the service;
5. prints how to reach the dashboard, the email and the generated password (shown once).

The service listens on `127.0.0.1:8090`, so from your own computer open a tunnel and then
<http://127.0.0.1:8090/_/>:

```sh
ssh -L 8090:127.0.0.1:8090 you@your-server
```

To serve it on a domain with HTTPS, put a reverse proxy in front: see
[deployment](deployment.md#reverse-proxy).

Settings, as environment variables before `sudo` (use `sudo env NAME=value sh`):

| Variable | |
|---|---|
| `SLONBASE_EMAIL`, `SLONBASE_PASSWORD` | the first superuser, so no question is asked (the password is generated if left out) |
| `SLONBASE_DATABASE_URL` | use this PostgreSQL database and install none here, e.g. a managed one: `postgres://user:pass@db.example.com:5432/app?sslmode=require` |
| `SLONBASE_DB_USER`, `SLONBASE_DB_NAME` | the names of the role and database it creates (both `slonbase`) |
| `SLONBASE_BASE_URL`, `SLONBASE_PACKAGE` | another repository, or a local `.deb` or `.rpm` file |

Without a terminal and without `SLONBASE_EMAIL` (a provisioning script, say) it installs
everything but the superuser, and prints the one-time link of the setup wizard instead. Running
it again on a machine that is already set up only installs the newest package.

## Step by step on Linux

What the quick install does, one command at a time, for when you want to choose each step. All
commands run on the server.

### 1. PostgreSQL

Skip this if you have a server already (then you only need step 2 on it, or a managed
database's connection URL).

**Debian, Ubuntu:**

```sh
sudo apt update
sudo apt install postgresql
```

**Fedora, RHEL, Rocky, Alma:**

```sh
sudo dnf module enable postgresql:16      # RHEL 8 and 9 only; the default stream is too old
sudo dnf install postgresql-server
sudo postgresql-setup --initdb
sudo systemctl enable --now postgresql
```

Check that it runs and is new enough (the number is 140000 or more):

```sh
sudo -u postgres psql -tAc 'SHOW server_version_num'
```

Red Hat's family allows only "ident" logins on localhost by default, which have no password.
In the file shown by `sudo -u postgres psql -tAc 'SHOW hba_file'`, change `ident` to
`scram-sha-256` on the two `host ... 127.0.0.1/32` and `::1/128` lines, then
`sudo systemctl reload postgresql`.

### 2. A role and a database for slonbase

```sh
PW=$(openssl rand -hex 24); echo "$PW"      # keep it: it goes into the connection URL
sudo -u postgres psql -c "CREATE ROLE slonbase LOGIN PASSWORD '$PW'" \
                      -c "CREATE DATABASE slonbase OWNER slonbase"
```

The role needs to create schemas and tables in its own database; owning the database is enough,
and it does **not** need to be a PostgreSQL superuser. The connection URL is then

```
postgres://slonbase:<the password>@127.0.0.1:5432/slonbase?sslmode=disable
```

(`sslmode=disable` is fine on the same machine. For a database on another machine use
`sslmode=require`, and a password without `@`, `:` or `/`, or percent-encoded.)

Try it before going on:

```sh
PGPASSWORD="$PW" psql -h 127.0.0.1 -U slonbase slonbase -c 'select 1'
```

### 3. Install slonbase

**Debian, Ubuntu:**

```sh
curl -fsSL https://slonbase.com/slonbase.gpg | sudo tee /usr/share/keyrings/slonbase.gpg > /dev/null
curl -fsSL https://slonbase.com/slonbase.list | sudo tee /etc/apt/sources.list.d/slonbase.list
sudo apt update && sudo apt install slonbase
```

**Fedora, RHEL, Rocky, Alma:**

```sh
sudo curl -fsSL -o /etc/yum.repos.d/slonbase.repo https://slonbase.com/slonbase.repo
sudo dnf install slonbase            # accept the signing key when asked
```

The repositories carry the newest three versions of every [supported release line](https://github.com/ayeshrodz/slonbase/blob/main/SECURITY.md#supported-versions)
(today 2.5.x and 2.4.x). To install an older one, or a package file
instead of the repository, download the `.deb` or `.rpm` for your architecture from the
[releases page](https://github.com/ayeshrodz/slonbase/releases), check it against `SHA256SUMS`
(`sha256sum --ignore-missing -c SHA256SUMS`, or see [Verify a download](#verify-a-download)) and install it with `sudo apt install ./slonbase_<version>_amd64.deb`
or `sudo dnf install ./slonbase-<version>-1.x86_64.rpm`.

The package installs a hardened systemd service and starts it, in **setup mode** (nothing is
configured yet). It installs:

| Path | |
|---|---|
| `/usr/bin/slonbase` | the binary |
| `/usr/lib/systemd/system/slonbase.service` | the service (user `slonbase`), enabled and started on install |
| `/etc/default/slonbase` | the service's environment (listen address, log level) |
| `/etc/slonbase/config.toml` | the config file, written by the setup (`0600`) |
| `/var/lib/slonbase/` | `storage/` (files), `backups/`, `hooks/`, `migrations/`, `projects/` |

### 4. Set it up

Either **in the browser**: get the one-time link and open it through an SSH tunnel (the service
listens on `127.0.0.1:8090` only):

```sh
sudo journalctl -u slonbase | grep "setup?token"       # prints http://127.0.0.1:8090/_/setup?token=...
ssh -L 8090:127.0.0.1:8090 you@your-server              # on your computer, then open the link
```

enter the connection URL from step 2, then the first superuser's email and a password.

Or **from the command line**, with no browser:

```sh
sudo systemctl stop slonbase
sudo -u slonbase env SLONBASE_SUPERUSER_PASSWORD='choose-a-password' slonbase setup \
    --config /etc/slonbase/config.toml \
    --database-url "postgres://slonbase:$PW@127.0.0.1:5432/slonbase?sslmode=disable" \
    --email you@example.com
sudo systemctl start slonbase
```

### 5. Check it, then secure it

[Check that it works](#check-that-it-works). Then, before real use: put a reverse proxy with
HTTPS in front ([deployment](deployment.md#reverse-proxy)), set the application's public address
in Settings, and set up [backups](deployment.md#backups).

To reach the service from other machines without a proxy (only on a network you trust), set
`SLONBASE_BIND=0.0.0.0:8090` in `/etc/default/slonbase` and `sudo systemctl restart slonbase`.

## Docker Compose

The quickest way if you have Docker and don't want to set up PostgreSQL yourself.
[`compose.yaml`](../compose.yaml) runs slonbase and PostgreSQL 17 together (the database, role and
password are created for you):

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

`SLONBASE_VERSION` in `.env` pins a release (`2.7.1`); the default is `latest`. When the image
can't be pulled, or with `docker compose up --build`, Compose builds it from a checkout of the
repository (run it there).

## Docker

The image `ghcr.io/ayeshrodz/slonbase` (amd64 and arm64) contains only the binary. It runs as an
unprivileged user (65532), listens on `0.0.0.0:8090`, keeps everything under the `/data` volume,
and has a health check (`slonbase health`).

```sh
docker run -d --name slonbase -p 8090:8090 -v slonbase_data:/data \
  -e SLONBASE_DATABASE_URL='postgres://user:pass@db.example.com:5432/app?sslmode=require' \
  ghcr.io/ayeshrodz/slonbase:2.7.1
docker logs slonbase 2>&1 | grep "setup?token"
```

Without `SLONBASE_DATABASE_URL` the setup wizard asks for the connection and saves it in
`/data/slonbase.toml`. JavaScript hooks go in `/data/hooks`.

## Plain binary

```sh
curl -LO https://github.com/ayeshrodz/slonbase/releases/download/v2.7.1/slonbase-2.7.1-linux-amd64.tar.gz
tar xzf slonbase-2.7.1-linux-amd64.tar.gz
./slonbase serve      # starts in setup mode and prints the setup link
```

By default it listens on `127.0.0.1:8090` and keeps its config (`slonbase.toml`) and data
(`slonbase_data/`, `slonbase_hooks/`) in the current directory; see
[configuration](configuration.md).

## macOS

**Experimental.** The archive is built for Apple silicon (M1 and later) and is **not signed or
notarized** (that needs a paid Apple developer account), so you check it yourself, as below. On an
Intel Mac, use [Docker](#docker) or [build from source](#from-source). Releases carry the archive
from 2.7.0: a release candidate lists it on its
[release page](https://github.com/ayeshrodz/slonbase/releases) when it has one.

You need a PostgreSQL 14 or newer first. With [Homebrew](https://brew.sh/):

```sh
brew install postgresql@16 && brew services start postgresql@16
"$(brew --prefix postgresql@16)/bin/createdb" slonbase   # an empty database, owned by you
```

Then download, check and start slonbase (`V` is the version, such as `2.7.1`):

```sh
V=2.7.1
curl -fsSLO https://github.com/ayeshrodz/slonbase/releases/download/v$V/slonbase-$V-macos-arm64.tar.gz
curl -fsSLO https://github.com/ayeshrodz/slonbase/releases/download/v$V/SHA256SUMS
grep "macos-arm64.tar.gz" SHA256SUMS | shasum -a 256 -c     # must say: OK
tar xzf slonbase-$V-macos-arm64.tar.gz
./slonbase serve                                            # prints the setup link
```

In the setup wizard use the database URL `postgres://YOUR-MAC-USER@localhost:5432/slonbase?sslmode=disable`
(Homebrew's PostgreSQL trusts local connections, so there is no password). `whoami` prints the
user name. The config (`slonbase.toml`) and the data are kept in the current directory.

- **"Cannot be opened because the developer cannot be verified".** macOS marks files downloaded
  with a browser. A file downloaded with `curl`, as above, isn't marked. After you have checked
  the checksum, you can clear the mark with `xattr -d com.apple.quarantine slonbase`, or allow it in
  System Settings → Privacy & Security → *Open Anyway*.
- **Is this the file the project built?** The checksum above matches the file to `SHA256SUMS`;
  with the [GitHub CLI](https://cli.github.com/), `gh attestation verify slonbase-$V-macos-arm64.tar.gz
  --repo ayeshrodz/slonbase` also shows that GitHub Actions built it from this repository (see
  [verify a download](#verify-a-download)).
- **Running it all the time.** No launchd file is provided yet: run `./slonbase serve` in a
  terminal, or write a launchd agent that does.

## Windows

**Experimental.** The archive is built for x64 Windows (10, 11 and Server 2019 or newer; Windows
on Arm runs it through its emulation) and is **not signed**, so Windows may ask before it runs it,
and you check the download yourself, as below. Releases carry the archive from 2.7.0: a release
candidate lists it on its [release page](https://github.com/ayeshrodz/slonbase/releases) when it
has one.

You need a PostgreSQL 14 or newer first, for example from
[postgresql.org](https://www.postgresql.org/download/windows/) or
`winget install PostgreSQL.PostgreSQL.16`. In its `psql` shell, make a role and an empty database
for slonbase:

```sql
CREATE ROLE slonbase LOGIN PASSWORD 'a-long-password';
CREATE DATABASE slonbase OWNER slonbase;
```

Then, in PowerShell, download, check and start slonbase (`$v` is the version, such as `2.7.1`):

```powershell
$v = "2.7.1"
$base = "https://github.com/ayeshrodz/slonbase/releases/download/v$v"
Invoke-WebRequest "$base/slonbase-$v-windows-amd64.zip" -OutFile slonbase.zip
Invoke-WebRequest "$base/SHA256SUMS" -OutFile SHA256SUMS
Select-String "windows-amd64.zip" SHA256SUMS      # the expected checksum
(Get-FileHash slonbase.zip -Algorithm SHA256).Hash # must be the same, in any case
Expand-Archive slonbase.zip -DestinationPath slonbase
cd slonbase
.\slonbase.exe serve                               # prints the setup link
```

In the setup wizard use the database URL
`postgres://slonbase:a-long-password@localhost:5432/slonbase?sslmode=disable`. The config
(`slonbase.toml`) and the data are kept in the current directory.

- **"Windows protected your PC" (SmartScreen).** Windows marks files downloaded with a browser.
  A file downloaded with `Invoke-WebRequest`, as above, isn't marked. After you have checked the
  checksum, choose *More info → Run anyway*, or clear the mark with `Unblock-File .\slonbase.exe`.
- **Is this the file the project built?** The checksum above matches the file to `SHA256SUMS`;
  with the [GitHub CLI](https://cli.github.com/), `gh attestation verify ..\slonbase.zip
  --repo ayeshrodz/slonbase` (run from the extracted folder) also shows that GitHub Actions built it from this repository (see
  [verify a download](#verify-a-download)).
- **What differs from Linux.** The dashboard's storage page doesn't show the free disk space.
  slonbase can't set the `0600` mode on the config file (it holds the database password), so keep
  it in a folder only you can read, such as your user profile. PostgreSQL is reached over TCP (no
  Unix sockets). No Windows service is provided yet: run it in a terminal, or with a tool that
  runs programs as a service.

## From source

Rust 1.89 or newer:

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
skips step 1. When there is no configuration file and no database URL (flag or environment
variable) at start, the process runs with the limits of a new installation on hook code (60
seconds, 128 MB, unless `--hooks-timeout`, `--hooks-memory` or their environment variables set
them) until it restarts. If the wizard then connects to an installation that already exists, the
file it saves has no hook limits, so after the next restart hooks run with the limits that file
sets (none unless you set `hooks.timeout_seconds` and `hooks.memory_limit_mb`, which the settings
warnings report). Restart once after connecting to an existing installation, and set the limits
in the file if you want them. Pointing a new server at a database that already has a slonbase installation
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
environment) the server uses; for the packages:
`sudo -u slonbase slonbase superuser upsert you@example.com 'new password' --config /etc/slonbase/config.toml`.

## Check that it works

```sh
curl -s http://127.0.0.1:8090/api/health               # {"code":200,"message":"API is healthy.",...}
sudo systemctl status slonbase                         # active (running); packages
sudo journalctl -u slonbase -n 50                      # its log
```

Then open <http://127.0.0.1:8090/_/> (through the SSH tunnel if the server is remote) and sign
in with the superuser's email and password. The overview shows the installation; **Health** in
the sidebar checks the database, storage and settings.

## Verify a download

Every release lists the SHA-256 of each file in `SHA256SUMS`:

```sh
sha256sum --ignore-missing -c SHA256SUMS                    # Linux
grep "macos-arm64.tar.gz" SHA256SUMS | shasum -a 256 -c    # macOS
```

On Windows, [`Get-FileHash`](#windows) prints the checksum to compare.

Releases after 2.5.1 also carry **attestations**, signed by GitHub, that say which workflow,
commit and run built each file, and a **software bill of materials**
(`slonbase-<version>.spdx.json`, SPDX 2.3): every Rust crate and bundled library in the binary,
with its version, license and checksum, for vulnerability scanners. With the
[GitHub CLI](https://cli.github.com/):

```sh
# this file was built by slonbase's release workflow, from this repository
gh attestation verify slonbase_<version>_amd64.deb --repo ayeshrodz/slonbase
# the same for the container image
gh attestation verify oci://ghcr.io/ayeshrodz/slonbase:<version> --repo ayeshrodz/slonbase
# the SBOM attached to a file
gh attestation verify slonbase_<version>_amd64.deb --repo ayeshrodz/slonbase \
  --predicate-type https://spdx.dev/Document/v2.3
```

The apt and dnf repositories are signed with the key published at
<https://slonbase.com/slonbase.asc> (fingerprint `9569 7FA7 3AFB AC43 E649 62F1 C7DF 885C 74BB 3942`).

## Troubleshooting

| You see | Usually |
|---|---|
| The wizard says it can't connect | The URL is wrong, or PostgreSQL isn't running (`sudo systemctl status postgresql`), or on a Red Hat system the localhost logins still use `ident` (step 1). Test the URL with `psql 'postgres://...'` first. |
| `password authentication failed for user "slonbase"` | The password in the URL is not the role's. Reset it with `sudo -u postgres psql -c "ALTER ROLE slonbase PASSWORD 'new'"` and enter the new URL. |
| `permission denied for database` or `for schema` | The role doesn't own the database: `sudo -u postgres psql -c 'ALTER DATABASE slonbase OWNER TO slonbase'`. |
| `database "slonbase" does not exist` | Step 2 wasn't done, or the name in the URL differs. |
| `address already in use` | Another program uses port 8090: change `SLONBASE_BIND` in `/etc/default/slonbase`. |
| The setup link is lost | `sudo journalctl -u slonbase \| grep "setup?token"`, or restart the service for a new token, or use the headless setup. |
| Setup mode is back after an upgrade | The config file wasn't found: the packages use `/etc/slonbase/config.toml`; a plain binary uses `slonbase.toml` in its working directory. |
| The password is forgotten | See *Lost superuser access* below. |
| Nothing answers from another machine | The service listens on `127.0.0.1` only (by design): use a tunnel or a reverse proxy. |

## Upgrading

Back up first (a slonbase backup, a database dump, or both; see
[deployment](deployment.md#backups)). Then:

- **Compose:** `docker compose pull && docker compose up -d` (or set `SLONBASE_VERSION`).
- **Docker:** run the new tag with the same volume and environment.
- **Packages:** `sudo apt update && sudo apt upgrade` or `sudo dnf upgrade` with the
  [repositories](#3-install-slonbase) (an installation from a package file
  can add them at any time), or install the new `.deb`/`.rpm` file. The service restarts with
  the new version, and `/etc/default/slonbase` and `/etc/slonbase/config.toml` are kept.
- **Binary:** replace the file and restart.

A new version applies its system migrations on start, in one transaction under an advisory lock,
so several instances starting at once apply them once, and a failed migration changes nothing.
Downgrading after a migration isn't supported: restore the backup instead. Every change is tested
for upgrades in place from the first release (0.1.0) and from the previous one: CI seeds the
old release with data, starts the new build on it, and checks the data, files, settings, old
passwords and tokens, rules, realtime and backups (`compat/upgrade/run.sh`).

**To 2.7.1:** if several instances share one database, stop all of them before you start the new
version: 2.7.1 adds one migration (it stores what each instance reports on Health), and rolling
upgrades aren't supported (see [several instances](deployment.md#several-instances)). A single
instance needs nothing more than the steps above.

**To 2.7:** stop every instance before starting the new version (2.7 stores settings and job
schedules in a form 2.6 doesn't read, so the two must not run side by side). Nothing else needs
doing: what worked on 2.6 works the same way on 2.7.

- **Configuration:** every new setting is off or keeps the old behaviour by default:
  `hooks.timeout_seconds` and `hooks.memory_limit_mb` (limits of hook code), `server.dev`
  (development mode). The connection budget is the same as 2.6's: the connections
  `database.job_connections` reserves for background jobs (2, or 1 of a project's) are part of
  `max_connections`, and requests use them too while no job runs.
- **Cron jobs** made in the dashboard become automations of one step, with their schedules,
  history and settings; `/api/crons` answers as before.
- **Failure notices:** a notice webhook keeps its address. An `https://` one is signed from its
  next save (its receiver may check the signature or ignore it); an `http://` one is sent
  unsigned, as before.
- **A job that calls an `http://` address with a credential header** keeps running; saving a
  change to it needs an `https://` address (or no such header).
- **Rust embedders:** `HooksConfig`, `ServerConfig` and `DatabaseConfig` have new fields; build them
  with `..Default::default()`.

The changelog's **Upgrading** section for 2.7.0 lists the same points.

**Several instances, to 1.7:** upgrade them all together (stop the old ones, then start the
new). Instances before 1.7 rely on a notification sent with each write to deliver realtime
events, which 1.7 no longer sends (see [several instances](deployment.md#several-instances)).

### Trying a release candidate

Before some releases, a release candidate (`2.7.0-rc.1`) is published on the
[releases page](https://github.com/ayeshrodz/slonbase/releases) as a pre-release. It is never in
the apt and dnf repositories and never the image's `latest`, so nothing upgrades to it on its own:
installing it is a choice. Try it on a copy of a real installation (restore a backup into a
spare server, say) rather than on the one people depend on, and back up first as for any upgrade.

- **Packages:** download the `.deb` or `.rpm` for your architecture from the candidate's
  release page, check it against its `SHA256SUMS`, and install it over the installed version
  with `sudo apt install ./slonbase_2.7.0-rc.1_amd64.deb` or
  `sudo dnf install ./slonbase-2.7.0-rc.1-1.x86_64.rpm`. The service restarts with it, and the
  configuration is kept.
- **Docker:** use the candidate's tag (`ghcr.io/ayeshrodz/slonbase:2.7.0-rc.1`, or
  `SLONBASE_VERSION=2.7.0-rc.1` with Compose).
- **Binary:** replace the file with the one from the candidate's archive and restart. A candidate
  after 2.7.0-rc.4 also has [macOS](#macos) and [Windows](#windows) archives.

Afterwards, `sudo apt upgrade` or `sudo dnf upgrade` moves an installation from the repositories
on to the release when it comes out (the package's version, `2.7.1~rc.1`, sorts before
`2.7.1`), and to a later candidate only if you install it the same way. Going back to the
previous release is the same as after any upgrade: restore the backup.

## Uninstalling

- **Packages:** `sudo apt remove slonbase` / `sudo dnf remove slonbase`. The config and
  `/var/lib/slonbase` are kept (`apt purge` removes them). To remove the repository too:
  `sudo rm /etc/apt/sources.list.d/slonbase.list /usr/share/keyrings/slonbase.gpg` or
  `sudo rm /etc/yum.repos.d/slonbase.repo`.
- **Docker:** remove the container and, if you no longer need it, the `/data` volume.

slonbase never drops the database. To remove its data from PostgreSQL:
`DROP SCHEMA slonbase CASCADE; DROP SCHEMA _slonbase CASCADE;`.

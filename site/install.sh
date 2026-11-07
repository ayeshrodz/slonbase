#!/bin/sh
# slonbase installer for Debian, Ubuntu, Fedora, RHEL, Rocky and Alma (amd64, arm64).
#
#   curl -fsSL https://slonbase.com/install.sh | sudo sh
#
# It installs PostgreSQL when the machine has none, creates a role and a database for
# slonbase, installs the signed slonbase package, and creates the first superuser. Every
# step is printed. It never changes an existing role's password or touches an existing
# database. It can be run again after a failure: the role it created and its password are
# kept in /etc/slonbase/install.env (root only) until setup succeeds, and a rerun reuses them. Read it before running it: it is short, and the manual steps it automates are in
# https://slonbase.com/docs/installation/
#
# Settings (environment variables, all optional):
#   SLONBASE_EMAIL          the first superuser's email (asked for when there is a terminal)
#   SLONBASE_PASSWORD       its password (default: a generated one, shown at the end)
#   SLONBASE_DATABASE_URL   use this PostgreSQL database instead of installing one here,
#                           e.g. postgres://user:pass@db.example.com:5432/app?sslmode=require
#   SLONBASE_DB_USER        the local role to create (default: slonbase)
#   SLONBASE_DB_NAME        the local database to create (default: slonbase)
#   SLONBASE_BASE_URL       where the repositories are (default: https://slonbase.com)
#   SLONBASE_PACKAGE        install this local .deb or .rpm instead of the repository
set -eu

BASE_URL=${SLONBASE_BASE_URL:-https://slonbase.com}
DB_USER=${SLONBASE_DB_USER:-slonbase}
DB_NAME=${SLONBASE_DB_NAME:-slonbase}
CONFIG=/etc/slonbase/config.toml

say() { printf '\n==> %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
die() {
    printf '\nerror: %s\n' "$*" >&2
    exit 1
}

# ------------------------------------------------------------------ checks
[ "$(id -u)" -eq 0 ] || die "run this as root: curl -fsSL $BASE_URL/install.sh | sudo sh"

if command -v apt-get >/dev/null 2>&1; then
    PM=apt
elif command -v dnf >/dev/null 2>&1; then
    PM=dnf
else
    die "this installer supports Debian, Ubuntu, Fedora, RHEL, Rocky and Alma (apt or dnf). For other systems see $BASE_URL/docs/installation/ (Docker, plain binary)."
fi

case $(uname -m) in
    x86_64 | amd64) ;;
    aarch64 | arm64) ;;
    *) die "slonbase packages exist for x86_64 and aarch64, not $(uname -m)." ;;
esac

SYSTEMD=no
[ -d /run/systemd/system ] && SYSTEMD=yes

case $DB_USER$DB_NAME in
    *[!a-z0-9_]* | '') die "SLONBASE_DB_USER and SLONBASE_DB_NAME may only contain a-z, 0-9 and _." ;;
esac

random_hex() { od -An -N"$1" -tx1 /dev/urandom | tr -d ' \n'; }

run_as() { # run_as <user> <command...>
    _user=$1
    shift
    # from the user's home: they can't always read the current directory
    ( cd "$(getent passwd "$_user" | cut -d: -f6)" && runuser -u "$_user" -- "$@" )
}

pg() { run_as postgres psql -X -qtA -v ON_ERROR_STOP=1 "$@"; }

start_service() { # start_service <name>
    if [ "$SYSTEMD" = yes ]; then
        systemctl enable "$1" >/dev/null 2>&1 || true
        systemctl start "$1"
    elif command -v service >/dev/null 2>&1; then
        service "$1" start >/dev/null 2>&1 || true
    fi
}

# ------------------------------------------------------------------ slonbase
# first, so that a failed download changes nothing in PostgreSQL
say "Installing slonbase"
if [ -n "${SLONBASE_PACKAGE:-}" ]; then
    case $PM in
        apt) apt-get install -y "$SLONBASE_PACKAGE" ;;
        dnf) dnf install -y "$SLONBASE_PACKAGE" ;;
    esac
elif [ "$PM" = apt ]; then
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -q
    apt-get install -y --no-install-recommends curl ca-certificates gnupg
    curl -fsSL "$BASE_URL/slonbase.gpg" -o /usr/share/keyrings/slonbase.gpg
    curl -fsSL "$BASE_URL/slonbase.list" -o /etc/apt/sources.list.d/slonbase.list
    apt-get update -q
    apt-get install -y --no-install-recommends slonbase
else
    curl -fsSL "$BASE_URL/slonbase.repo" -o /etc/yum.repos.d/slonbase.repo
    dnf install -y slonbase
fi
command -v slonbase >/dev/null 2>&1 || die "the slonbase package did not install."
note "$(slonbase --version)"

# ------------------------------------------------------------------ PostgreSQL
DATABASE_URL=${SLONBASE_DATABASE_URL:-}
DB_PASSWORD=
CONFIGURED=no
[ -s "$CONFIG" ] && grep -q '^url' "$CONFIG" 2>/dev/null && CONFIGURED=yes

if [ "$CONFIGURED" = yes ]; then
    say "slonbase is already set up here ($CONFIG): only installing the newest package"
elif [ -n "$DATABASE_URL" ]; then
    say "Using the PostgreSQL database from SLONBASE_DATABASE_URL"
    note "No PostgreSQL is installed on this machine."
else
    have_server() {
        ls /usr/lib/postgresql/*/bin/postgres >/dev/null 2>&1 \
            || ls /usr/pgsql-*/bin/postgres >/dev/null 2>&1 \
            || command -v postgres >/dev/null 2>&1 \
            || [ -x /usr/bin/postgresql-setup ]
    }

    if have_server; then
        say "PostgreSQL is already installed"
    else
        say "Installing PostgreSQL"
        if [ "$PM" = apt ]; then
            export DEBIAN_FRONTEND=noninteractive
            apt-get update -q
            apt-get install -y postgresql
        else
            # RHEL 8 and 9 ship PostgreSQL as module streams, and 9's default (13) is too old
            dnf module enable -y postgresql:16 >/dev/null 2>&1 || true
            dnf install -y postgresql-server
            if [ ! -s /var/lib/pgsql/data/PG_VERSION ]; then
                postgresql-setup --initdb
            fi
        fi
    fi

    start_service postgresql
    # a freshly started server takes a moment to accept connections
    tries=0
    until pg -d postgres -c 'SELECT 1' >/dev/null 2>&1; do
        tries=$((tries + 1))
        [ "$tries" -lt 30 ] || die "PostgreSQL does not answer. Check: systemctl status postgresql"
        sleep 1
    done

    version=$(pg -d postgres -c 'SHOW server_version_num')
    [ "$version" -ge 140000 ] 2>/dev/null \
        || die "PostgreSQL $version is older than 14, which slonbase needs. Install a newer one from https://www.postgresql.org/download/linux/ (or use SLONBASE_DATABASE_URL) and run this again."
    note "PostgreSQL server_version_num $version is running."

    say "Creating the database role and database"
    STATE=/etc/slonbase/install.env
    role_exists=$(pg -d postgres -c "SELECT 1 FROM pg_roles WHERE rolname = '$DB_USER'")
    db_exists=$(pg -d postgres -c "SELECT 1 FROM pg_database WHERE datname = '$DB_NAME'")
    if [ -s "$STATE" ]; then
        # an earlier run created them and stopped before setup finished: reuse its password
        saved_user=$(sed -n 's/^DB_USER=//p' "$STATE")
        saved_name=$(sed -n 's/^DB_NAME=//p' "$STATE")
        DB_PASSWORD=$(sed -n 's/^DB_PASSWORD=//p' "$STATE")
        [ "$saved_user" = "$DB_USER" ] && [ "$saved_name" = "$DB_NAME" ] && [ -n "$DB_PASSWORD" ] \
            || die "$STATE is from an install of role \"$saved_user\" and database \"$saved_name\". Run with SLONBASE_DB_USER=$saved_user SLONBASE_DB_NAME=$saved_name, or remove $STATE."
        note "Continuing an earlier install (the role's password is in $STATE)."
    elif [ "$role_exists" = 1 ]; then
        die "the PostgreSQL role \"$DB_USER\" already exists, and its password is not mine to change. Use another name (SLONBASE_DB_USER=...), or point slonbase at your own database with SLONBASE_DATABASE_URL=postgres://user:pass@127.0.0.1:5432/dbname"
    elif [ "$db_exists" = 1 ]; then
        die "the PostgreSQL database \"$DB_NAME\" already exists. Use another name (SLONBASE_DB_NAME=...), or point slonbase at it with SLONBASE_DATABASE_URL=..."
    else
        DB_PASSWORD=$(random_hex 24)
        # written before the role exists, so the password survives whatever fails next
        mkdir -p /etc/slonbase
        ( umask 077 && printf 'DB_USER=%s\nDB_NAME=%s\nDB_PASSWORD=%s\n' "$DB_USER" "$DB_NAME" "$DB_PASSWORD" >"$STATE" )
    fi
    if [ "$role_exists" != 1 ]; then
        pg -d postgres -c "CREATE ROLE \"$DB_USER\" LOGIN PASSWORD '$DB_PASSWORD'"
    fi
    if [ "$db_exists" != 1 ]; then
        pg -d postgres -c "CREATE DATABASE \"$DB_NAME\" OWNER \"$DB_USER\""
    fi
    note "Role \"$DB_USER\" and database \"$DB_NAME\" are ready (a generated password, kept in $CONFIG)."

    # Red Hat's default pg_hba.conf has TCP logins on localhost use "ident", which has no
    # password. Rather than change those lines for everyone, put password rules for this role
    # and database first (the first matching line wins)
    if [ "$PM" = dnf ]; then
        hba=$(pg -d postgres -c 'SHOW hba_file')
        if ! grep -q "^host[[:space:]]\{1,\}${DB_NAME}[[:space:]]\{1,\}${DB_USER}[[:space:]]" "$hba"; then
            cp -p "$hba" "$hba.slonbase.bak"
            {
                printf '# slonbase: password logins for its own role and database\n'
                printf 'host %s %s 127.0.0.1/32 scram-sha-256\n' "$DB_NAME" "$DB_USER"
                printf 'host %s %s ::1/128 scram-sha-256\n' "$DB_NAME" "$DB_USER"
                cat "$hba.slonbase.bak"
            } >"$hba"
            pg -d postgres -c 'SELECT pg_reload_conf()' >/dev/null
            note "Password logins for \"$DB_USER\" on localhost added to $hba (the original is $hba.slonbase.bak)."
        fi
    fi

    port=$(pg -d postgres -c 'SHOW port')
    DATABASE_URL="postgres://$DB_USER:$DB_PASSWORD@127.0.0.1:$port/$DB_NAME?sslmode=disable"
fi

# ------------------------------------------------------------------ first superuser
EMAIL=${SLONBASE_EMAIL:-}
[ "$CONFIGURED" = yes ] && EMAIL=
if [ "$CONFIGURED" = no ] && [ -z "$EMAIL" ] && (: </dev/tty) 2>/dev/null; then
    printf '\nEmail for the first superuser: ' >/dev/tty
    read -r EMAIL </dev/tty || EMAIL=
fi

if [ -n "$EMAIL" ]; then
    say "Creating the first superuser"
    PASSWORD=${SLONBASE_PASSWORD:-$(random_hex 12)}
    # the service is in setup mode (no database yet); setup writes the configuration itself
    if [ "$SYSTEMD" = yes ]; then
        systemctl stop slonbase >/dev/null 2>&1 || true
    fi
    run_as slonbase env RUST_LOG=warn SLONBASE_SUPERUSER_PASSWORD="$PASSWORD" \
        slonbase setup --config "$CONFIG" --database-url "$DATABASE_URL" --email "$EMAIL"
    # the password is in the configuration now
    rm -f /etc/slonbase/install.env
    if [ "$SYSTEMD" = yes ]; then
        systemctl start slonbase
        tries=0
        until curl -fsS http://127.0.0.1:8090/api/health >/dev/null 2>&1; do
            tries=$((tries + 1))
            [ "$tries" -lt 30 ] || die "slonbase does not answer on 127.0.0.1:8090. Check: journalctl -u slonbase"
            sleep 1
        done
    fi
fi

# ------------------------------------------------------------------ done
say "slonbase is installed"
if [ -n "$EMAIL" ]; then
    note "Dashboard:  http://127.0.0.1:8090/_/   (from another machine: ssh -L 8090:127.0.0.1:8090 <this-server>)"
    note "Email:      $EMAIL"
    if [ -z "${SLONBASE_PASSWORD:-}" ]; then
        note "Password:   $PASSWORD   (generated; shown only now. Change it in Settings > Team)"
    fi
elif [ "$CONFIGURED" = yes ]; then
    if [ "$SYSTEMD" = yes ]; then
        systemctl restart slonbase >/dev/null 2>&1 || true
    fi
    note "Your configuration and data were left as they are."
else
    note "Finish in the browser with the one-time link:"
    note "    journalctl -u slonbase | grep 'setup?token'"
    if [ -n "$DB_PASSWORD" ]; then
        note "The setup wizard asks for the database. Enter:"
        note "    $DATABASE_URL"
    fi
fi
note "Config:     $CONFIG   Files and backups: /var/lib/slonbase"
note "Next:        put a reverse proxy with HTTPS in front  ->  $BASE_URL/docs/deployment/"
if [ -z "${SLONBASE_PACKAGE:-}" ]; then
    note "Upgrade:     apt upgrade / dnf upgrade (the repository was added)"
fi
if [ "$SYSTEMD" != yes ]; then
    note "This machine has no systemd, so nothing was started. Run: slonbase serve --config $CONFIG"
fi

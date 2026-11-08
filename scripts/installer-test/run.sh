#!/bin/sh
# Runs site/install.sh in a clean container and checks the cases that used to go wrong:
#
#   A. the package can't be installed: nothing is created in PostgreSQL;
#   B. setup fails after the role exists: a rerun reuses the role and its saved password;
#   C. a rerun on an installed system only upgrades the package and keeps the configuration;
#   and the installed service starts and answers /api/health, as the slonbase user.
#
#   scripts/installer-test/run.sh <image> <package v1> <package v2>
#   scripts/installer-test/run.sh ubuntu:24.04 dist/old.deb dist/new.deb
#
# Needs Docker. Containers have no systemd, so on dnf images PostgreSQL is started here (a real
# server's systemd does that); on apt images the installer starts it with `service`.
set -eu

image=${1:?usage: run.sh <image> <package v1> <package v2>}
v1=${2:?package v1}
v2=${3:?package v2}
root=$(cd "$(dirname "$0")/../.." && pwd)

docker run --rm -i -v "$root:/src:ro" -v "$(cd "$(dirname "$v1")" && pwd):/pkg:ro" \
    -e V1="/pkg/$(basename "$v1")" -e V2="/pkg/$(basename "$v2")" "$image" sh -eu <<'INNER'
step() { printf '\n######## %s\n' "$*"; }
fail() { printf '\nFAIL: %s\n' "$*" >&2; exit 1; }
installer() { sh /src/site/install.sh; }

# container images leave out /usr/share/doc to save space; a server keeps it, and the package's
# license files live there
if [ -d /etc/dpkg/dpkg.cfg.d ]; then
    sed -i '/usr\/share\/doc/d' /etc/dpkg/dpkg.cfg.d/* 2>/dev/null || true
fi
if [ -f /etc/dnf/dnf.conf ]; then
    sed -i '/^tsflags=nodocs/d' /etc/dnf/dnf.conf
fi

if command -v apt-get >/dev/null 2>&1; then
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -q >/dev/null && apt-get install -y -q curl ca-certificates >/dev/null
    psql_as() { su postgres -s /bin/sh -c "psql -X -qtA -c \"$1\""; }
else
    dnf install -y -q curl postgresql-server util-linux >/dev/null
    # Red Hat's defaults: peer on the socket, ident (no password) on localhost TCP
    su postgres -s /bin/sh -c 'initdb -D /var/lib/pgsql/data --auth-local=peer --auth-host=ident >/dev/null'
    # systemd-tmpfiles creates the socket directory at boot; there is no boot here
    mkdir -p /run/postgresql && chown postgres:postgres /run/postgresql
    su postgres -s /bin/sh -c 'pg_ctl -D /var/lib/pgsql/data -l /tmp/pg.log -w start >/dev/null' \
        || { cat /tmp/pg.log; fail "PostgreSQL didn't start"; }
    psql_as() { su postgres -s /bin/sh -c "psql -X -qtA -c \"$1\""; }
fi
export SLONBASE_DB_USER=itest SLONBASE_DB_NAME=itest

step "A. the package can't be installed"
if SLONBASE_PACKAGE=/nonexistent.pkg SLONBASE_EMAIL=a@example.com installer >/tmp/a.log 2>&1; then
    fail "the installer succeeded without a package"
fi
# PostgreSQL may not even be installed yet (the package comes first): then nothing was created
if command -v psql >/dev/null 2>&1 && [ "$(psql_as "SELECT count(*) FROM pg_roles WHERE rolname = 'itest'" 2>/dev/null || echo 0)" != 0 ]; then
    fail "a role was created although the package failed"
fi
[ ! -e /etc/slonbase/install.env ] || fail "a state file was written although the package failed"
echo "ok: nothing created"

step "B1. setup fails after the role exists"
if SLONBASE_PACKAGE="$V1" SLONBASE_EMAIL=not-an-email installer >/tmp/b1.log 2>&1; then
    cat /tmp/b1.log; fail "setup with an invalid email succeeded"
fi
[ "$(psql_as "SELECT count(*) FROM pg_roles WHERE rolname = 'itest'")" = 1 ] || { cat /tmp/b1.log; fail "the role wasn't created"; }
[ "$(stat -c %a /etc/slonbase/install.env)" = 600 ] || fail "the state file isn't private"
echo "ok: role kept, password saved privately"

step "B2. the rerun reuses the role"
SLONBASE_PACKAGE="$V1" SLONBASE_EMAIL=owner@example.com SLONBASE_PASSWORD=owner-password-1 installer >/tmp/b2.log 2>&1 \
    || { cat /tmp/b2.log; fail "the rerun failed"; }
grep -q "Continuing an earlier install" /tmp/b2.log || fail "the rerun didn't reuse the saved password"
[ ! -e /etc/slonbase/install.env ] || fail "the state file outlived setup"
grep -q '^url' /etc/slonbase/config.toml || fail "no database in the configuration"
echo "ok: installed"

serve_check() {
    su slonbase -s /bin/sh -c 'slonbase serve --config /etc/slonbase/config.toml >/tmp/serve.log 2>&1 &'
    i=0
    until curl -fsS http://127.0.0.1:8090/api/health >/dev/null 2>&1; do
        i=$((i + 1)); [ "$i" -lt 60 ] || { cat /tmp/serve.log; fail "slonbase didn't answer"; }
        sleep 1
    done
    curl -fsS -X POST http://127.0.0.1:8090/api/collections/_superusers/auth-with-password \
        -H 'content-type: application/json' \
        -d '{"identity":"owner@example.com","password":"owner-password-1"}' | grep -q '"token"' \
        || fail "the superuser can't sign in"
    pkill -f 'slonbase serve' || true
    sleep 1
}
step "the service starts as the slonbase user"
[ "$(stat -c %U /etc/slonbase/config.toml)" = slonbase ] || fail "the configuration isn't the service's"
serve_check
echo "ok: health and sign-in"

step "C. a rerun upgrades the package and keeps the configuration"
before=$(sha256sum /etc/slonbase/config.toml)
SLONBASE_PACKAGE="$V2" installer >/tmp/c.log 2>&1 || { cat /tmp/c.log; fail "the upgrade failed"; }
grep -q "already set up" /tmp/c.log || fail "the rerun didn't see the existing install"
[ "$(sha256sum /etc/slonbase/config.toml)" = "$before" ] || fail "the configuration changed"
for f in LICENSE NOTICE THIRD-PARTY-LICENSES.md; do
    [ -s "/usr/share/doc/slonbase/$f" ] || fail "/usr/share/doc/slonbase/$f is missing"
done
serve_check
echo "ok: upgraded, configuration kept, data still there"

printf '\nall installer checks passed on %s\n' "$(. /etc/os-release && echo "$PRETTY_NAME")"
INNER

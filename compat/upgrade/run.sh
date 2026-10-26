#!/usr/bin/env bash
# The upgrade from a released slonbase to this build: sets up the release on an empty
# database, seeds it through the SDK and snapshots it, stops it, starts this build on the same
# database, storage and config, and checks the snapshot (and that writes, sign-ins, tokens
# and realtime still work).
#
#   compat/upgrade/run.sh 0.1.0      a release version, or "previous" for the newest release
#                                    before this build's version
#
# Needs DATABASE_URL (an empty database) and a built target/debug/slonbase (or SLONBASE_BIN).
# SLONBASE_FROM_BIN uses a local binary of the old version instead of downloading it.
set -euo pipefail
cd "$(dirname "$0")/.."

from="${1:?usage: run.sh <version>|previous}"
: "${DATABASE_URL:?DATABASE_URL must point at an empty database}"
bin="$(realpath "${SLONBASE_BIN:-../target/debug/slonbase}")"
port="${COMPAT_PORT:-8099}"
export COMPAT_URL="http://127.0.0.1:${port}"
export COMPAT_EMAIL="upgrade-admin@example.com"
export COMPAT_PASSWORD="upgrade-admin-password-1"
repo="https://github.com/ayeshrodz/slonbase"

current="$("$bin" --version | awk '{print $2}')"
if [ "$from" = previous ]; then
  # the newest release older than this build (releases are listed newest first)
  auth=()
  [ -n "${GH_TOKEN:-}" ] && auth=(-H "Authorization: Bearer $GH_TOKEN")
  from="$(curl -fsSL "${auth[@]}" "https://api.github.com/repos/ayeshrodz/slonbase/releases?per_page=100" \
    | node -e '
      const current = process.argv[1].split(".").map(Number);
      const older = (v) => { for (let i = 0; i < 3; i++) if (v[i] !== current[i]) return v[i] < current[i]; return false; };
      let body = ""; process.stdin.on("data", (d) => (body += d)).on("end", () => {
        const versions = JSON.parse(body).filter((r) => !r.draft && !r.prerelease)
          .map((r) => r.tag_name.replace(/^v/, "").split(".").map(Number)).filter(older)
          .sort((a, b) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2]);
        if (!versions.length) process.exit(1);
        console.log(versions.pop().join("."));
      });' "$current")"
fi
echo "upgrading $from -> $current"
work="$(pwd)/.run/upgrade-$from"
rm -rf "$work" && mkdir -p "$work/old"

old="${SLONBASE_FROM_BIN:-}"
if [ -z "$old" ]; then
  tarball="slonbase-${from}-linux-amd64.tar.gz"
  (cd "$work/old" && curl -fsSLO "$repo/releases/download/v$from/$tarball" \
    && curl -fsSL "$repo/releases/download/v$from/SHA256SUMS" | grep " $tarball\$" | sha256sum -c - > /dev/null \
    && tar xzf "$tarball")
  old="$work/old/slonbase"
fi

wait_up() {
  for _ in $(seq 1 150); do
    curl -sf "${COMPAT_URL}/api/health" > /dev/null && return 0
    sleep 0.2
  done
  echo "the server didn't start:" >&2
  cat "$1" >&2
  exit 1
}
stop() {
  kill "$1" 2> /dev/null || true
  wait "$1" 2> /dev/null || true
}

# options every release has; everything lives in the work directory
common=(--config "$work/slonbase.toml" --storage-path "$work/storage" --backups-path "$work/backups"
  --hooks-dir "$work/hooks" --sql-console)
mkdir -p "$work/hooks"
cd "$work"

# 1. the old release, with data
"$old" "${common[@]}" setup --database-url "$DATABASE_URL" --email "$COMPAT_EMAIL" --password "$COMPAT_PASSWORD" > old-setup.log 2>&1
"$old" "${common[@]}" --bind "127.0.0.1:${port}" serve > old.log 2>&1 &
server=$!
trap 'stop $server' EXIT
wait_up old.log
node "$OLDPWD/upgrade/seed.mjs" snapshot.json
stop "$server"

# 2. this build on the same database, storage and config (it migrates on start)
"$bin" "${common[@]}" --migrations-dir "$work/migrations" --projects-path "$work/projects" \
  --bind "127.0.0.1:${port}" serve > new.log 2>&1 &
server=$!
wait_up new.log
# (before it's up, the health check answers 503 while it migrates)
started=$(wc -l < new.log)
SNAPSHOT="$work/snapshot.json" node --experimental-eventsource --test "$OLDPWD/upgrade/check.mjs"
if tail -n +"$((started + 1))" new.log | grep -E "ERROR|panicked"; then
  echo "the upgraded server logged errors (above)" >&2
  exit 1
fi

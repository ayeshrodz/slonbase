#!/usr/bin/env bash
# The PocketBase import, end to end: seeds a fresh PocketBase through the SDK and snapshots it,
# stops it, imports its pb_data into a fresh slonbase (set up with another superuser), and
# checks the snapshot against slonbase's API.
#
#   compat/import/run.sh     needs POCKETBASE_BIN, DATABASE_URL (an empty database) and a
#                            built target/debug/slonbase (or SLONBASE_BIN)
set -euo pipefail
cd "$(dirname "$0")/.."

: "${POCKETBASE_BIN:?POCKETBASE_BIN must point at a pocketbase binary}"
: "${DATABASE_URL:?DATABASE_URL must point at an empty database}"
bin="${SLONBASE_BIN:-../target/debug/slonbase}"
port="${COMPAT_PORT:-8099}"
export COMPAT_URL="http://127.0.0.1:${port}"
export COMPAT_EMAIL="pb-admin@example.com"
export COMPAT_PASSWORD="pb-admin-password-1"
work="$(pwd)/.run/import"
rm -rf "$work" && mkdir -p "$work"

wait_up() {
  for _ in $(seq 1 100); do
    curl -sf "${COMPAT_URL}/api/health" > /dev/null && return 0
    sleep 0.2
  done
  echo "the server didn't start:" >&2
  cat "$1" >&2
  exit 1
}

# 1. PocketBase with data
"$POCKETBASE_BIN" superuser upsert "$COMPAT_EMAIL" "$COMPAT_PASSWORD" --dir "$work/pb_data" > /dev/null
# with automigrate, the seeding also writes migration files (and marks them applied)
"$POCKETBASE_BIN" serve --dir "$work/pb_data" --http "127.0.0.1:${port}" \
  --automigrate --migrationsDir "$work/pb_migrations" > "$work/pocketbase.log" 2>&1 &
pb=$!
trap 'kill $pb 2>/dev/null || true' EXIT
wait_up "$work/pocketbase.log"
node import/seed-pocketbase.mjs "$work/snapshot.json"
kill "$pb" && wait "$pb" 2> /dev/null || true

# 2. the import into a slonbase with its own superuser
common=(--config "$work/slonbase.toml" --storage-path "$work/storage" --backups-path "$work/backups" --hooks-dir "$work/hooks" --migrations-dir "$work/pb_migrations" --projects-path "$work/projects")
"$bin" "${common[@]}" setup --database-url "$DATABASE_URL" --email slon-admin@example.com --password slon-admin-password-1 > /dev/null
# from PocketBase's backup zip
"$bin" "${common[@]}" import-pocketbase "$work/pb_data/backups/import-test.zip"
# importing again (here from the directory) is refused: the collections exist now
if "$bin" "${common[@]}" import-pocketbase "$work/pb_data" > "$work/again.log" 2>&1; then
  echo "a second import should have been refused" >&2
  exit 1
fi
grep -q "already has collections" "$work/again.log"
# PocketBase's applied migrations came along, so its pb_migrations don't run again
"$bin" "${common[@]}" migrate list > "$work/migrations.log" 2> /dev/null
cat "$work/migrations.log"
if grep -q "^pending" "$work/migrations.log" || ! grep -q "^applied" "$work/migrations.log"; then
  echo "PocketBase's migrations should all be marked applied" >&2
  exit 1
fi

# 3. the checks, against slonbase
"$bin" "${common[@]}" --bind "127.0.0.1:${port}" serve > "$work/slonbase.log" 2>&1 &
server=$!
trap 'kill $server 2>/dev/null || true; wait $server 2>/dev/null || true' EXIT
wait_up "$work/slonbase.log"
SNAPSHOT="$work/snapshot.json" node --test import/check-slonbase.mjs

# 4. PocketBase's migration files, applied to a new database (a project), give the same
# collections; the SQLite query of one view is ported to PostgreSQL first, as a user would
export REPLAY_PROJECT=replay REPLAY_EMAIL=replay@example.com REPLAY_PASSWORD=replay-password-1
SLONBASE_SUPERUSER_PASSWORD="$REPLAY_PASSWORD" "$bin" "${common[@]}" project create "$REPLAY_PROJECT" --email "$REPLAY_EMAIL" > /dev/null
mkdir -p "$work/projects/$REPLAY_PROJECT/migrations"
cp "$work"/pb_migrations/*.js "$work/projects/$REPLAY_PROJECT/migrations/"
sed -i "s/strftime('%Y', created)/to_char(created, 'YYYY')/" "$work/projects/$REPLAY_PROJECT/migrations/"*_yearly.js
# (the running server may have applied them already: it starts new projects within moments)
"$bin" "${common[@]}" --project "$REPLAY_PROJECT" migrate up 2> /dev/null
node --test import/check-replay.mjs

#!/usr/bin/env bash
# Starts a fresh slonbase and runs the dashboard checks (check.mjs) against it:
#
#   DATABASE_URL='postgres://...an empty database...' ui/test/run.sh [--update]
#
# SLONBASE_BIN (default target/debug/slonbase), UI_PORT (8095). Needs `npm ci` here and
# Playwright's Chromium (`npx playwright install chromium`).
set -euo pipefail
cd "$(dirname "$0")"
: "${DATABASE_URL:?DATABASE_URL must point at an empty database}"
bin="$(realpath "${SLONBASE_BIN:-../../target/debug/slonbase}")"
port="${UI_PORT:-8095}"
if curl -s -o /dev/null "http://127.0.0.1:$port/"; then
  echo "something already listens on port $port" >&2
  exit 1
fi
work="$(pwd)/.run"
rm -rf "$work" && mkdir -p "$work"
common=(--config "$work/slonbase.toml" --storage-path "$work/storage" --backups-path "$work/backups" --hooks-dir "$work/hooks")
export SB_EMAIL=admin@example.com SB_PASSWORD=admin-password-1
"$bin" "${common[@]}" setup --database-url "$DATABASE_URL" --email "$SB_EMAIL" --password "$SB_PASSWORD" > "$work/setup.log" 2>&1
"$bin" "${common[@]}" --bind "127.0.0.1:$port" serve > "$work/server.log" 2>&1 &
server=$!
trap 'kill $server 2>/dev/null || true; wait $server 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do
  curl -sf "http://127.0.0.1:$port/api/health" > /dev/null && break
  sleep 0.2
done
node check.mjs "http://127.0.0.1:$port" "$@"

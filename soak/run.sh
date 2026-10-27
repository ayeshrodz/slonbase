#!/usr/bin/env bash
# Soak test: starts INSTANCES slonbase instances (default 2) on one empty database, sharing
# their storage, and runs soak.mjs against all of them (see README.md).
#
#   DATABASE_URL='postgres://...an empty database...' soak/run.sh
#
# SLONBASE_BIN (default target/release/slonbase), INSTANCES, SOAK_PORT (first port, 8097), and
# soak.mjs's SOAK_* variables (SOAK_DURATION, SOAK_CLIENTS, ...).
set -euo pipefail
cd "$(dirname "$0")"

: "${DATABASE_URL:?DATABASE_URL must point at an empty database}"
bin="$(realpath "${SLONBASE_BIN:-../target/release/slonbase}")"
instances="${INSTANCES:-2}"
port="${SOAK_PORT:-8097}"
export SOAK_EMAIL="soak-admin@example.com"
export SOAK_PASSWORD="soak-admin-password-1"
work="$(pwd)/.run"
rm -rf "$work" && mkdir -p "$work"
common=(--config "$work/slonbase.toml" --storage-path "$work/storage" --backups-path "$work/backups" --hooks-dir "$work/hooks")

"$bin" "${common[@]}" setup --database-url "$DATABASE_URL" --email "$SOAK_EMAIL" --password "$SOAK_PASSWORD" > "$work/setup.log" 2>&1

pids=()
urls=()
metrics=()
trap 'for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; wait 2>/dev/null || true' EXIT
for i in $(seq 0 $((instances - 1))); do
  http=$((port + i * 2))
  metric=$((port + i * 2 + 1))
  "$bin" "${common[@]}" --bind "127.0.0.1:${http}" --metrics-bind "127.0.0.1:${metric}" serve \
    > "$work/server-$i.log" 2>&1 &
  pids+=("$!")
  urls+=("http://127.0.0.1:${http}")
  metrics+=("http://127.0.0.1:${metric}/metrics")
done
for url in "${urls[@]}"; do
  for _ in $(seq 1 150); do
    curl -sf "$url/api/health" > /dev/null && break
    sleep 0.2
  done
  if ! curl -sf "$url/api/health" > /dev/null; then
    echo "an instance didn't start:" >&2
    cat "$work"/server-*.log >&2
    exit 1
  fi
done

join() { local IFS=,; echo "$*"; }
status=0
SOAK_URLS="$(join "${urls[@]}")" SOAK_METRICS="$(join "${metrics[@]}")" SOAK_PIDS="$(join "${pids[@]}")" \
  SOAK_RESULT="${SOAK_RESULT:-$work/result.json}" node soak.mjs || status=$?

# errors the instances logged
# (failed requests are counted by soak.mjs; the 503s answered while starting aren't errors)
if grep -h 'ERROR' "$work"/server-*.log | grep -v 'tower_http::trace::on_failure' > "$work/errors.log"; then
  echo "the instances logged errors:" >&2
  head -20 "$work/errors.log" >&2
  status=1
fi
exit "$status"

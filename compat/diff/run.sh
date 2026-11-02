#!/usr/bin/env bash
# Starts PocketBase and slonbase side by side on fresh data stores and compares their answers
# to the same requests (diff.mjs), with fuzzed HTML for the excerpts.
#
#   compat/diff/run.sh     needs POCKETBASE_BIN, DATABASE_URL (an empty database slonbase may
#                          use) and a built target/debug/slonbase (or SLONBASE_BIN)
#
# FUZZ (default 1000) sets how many random HTML documents are compared, FUZZ_SEED their sequence.
set -euo pipefail
cd "$(dirname "$0")"
export COMPAT_EMAIL="${COMPAT_EMAIL:-admin@example.com}"
export COMPAT_PASSWORD="${COMPAT_PASSWORD:-admin-password-1}"
pb_url="http://127.0.0.1:${PB_PORT:-8091}"
sb_url="http://127.0.0.1:${SB_PORT:-8092}"
work="$(pwd)/../.run/diff"
# a server left on a port would answer in place of the fresh one
for url in "$pb_url" "$sb_url"; do
  if curl -s -o /dev/null "$url/api/health"; then echo "something already listens at $url" >&2; exit 1; fi
done
rm -rf "$work" && mkdir -p "$work"

pb="${POCKETBASE_BIN:?POCKETBASE_BIN must point at a pocketbase binary}"
sb="${SLONBASE_BIN:-../../target/debug/slonbase}"
: "${DATABASE_URL:?DATABASE_URL must point at an empty database}"

"$pb" superuser upsert "$COMPAT_EMAIL" "$COMPAT_PASSWORD" --dir "$work/pb_data" > "$work/pb-setup.log" 2>&1
"$pb" serve --dir "$work/pb_data" --http "${pb_url#http://}" > "$work/pb.log" 2>&1 &
pb_pid=$!
common=(--config "$work/slonbase.toml" --storage-path "$work/storage" --backups-path "$work/backups" --hooks-dir "$work/hooks")
"$sb" "${common[@]}" setup --database-url "$DATABASE_URL" --email "$COMPAT_EMAIL" --password "$COMPAT_PASSWORD" > "$work/sb-setup.log" 2>&1
"$sb" "${common[@]}" --bind "${sb_url#http://}" serve > "$work/sb.log" 2>&1 &
sb_pid=$!
stop() {
  kill "$pb_pid" "$sb_pid" 2>/dev/null || true
  wait "$pb_pid" "$sb_pid" 2>/dev/null || true
}
trap stop EXIT

for url in "$pb_url" "$sb_url"; do
  for _ in $(seq 1 100); do curl -sf "$url/api/health" > /dev/null && break; sleep 0.2; done
  curl -sf "$url/api/health" > /dev/null || { echo "$url didn't start" >&2; tail -20 "$work"/*.log >&2; exit 1; }
done

# the same settings on both: batch requests on (the test seeds with them), no rate limits
for url in "$pb_url" "$sb_url"; do
  node --input-type=module -e '
    const [url, identity, password] = process.argv.slice(1);
    const send = async (method, path, body, token) => {
      const res = await fetch(url + path, { method, headers: { "content-type": "application/json", ...(token ? { authorization: token } : {}) }, body: JSON.stringify(body) });
      if (!res.ok) throw new Error(`${method} ${path}: ${res.status} ${await res.text()}`);
      return res.json();
    };
    const { token } = await send("POST", "/api/collections/_superusers/auth-with-password", { identity, password });
    await send("PATCH", "/api/settings", { batch: { enabled: true, maxRequests: 50 }, rateLimits: { enabled: false } }, token);
  ' "$url" "$COMPAT_EMAIL" "$COMPAT_PASSWORD"
done

node diff.mjs "$pb_url" "$sb_url" --seed --fuzz "${FUZZ:-1000}" "$@"

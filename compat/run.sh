#!/usr/bin/env bash
# Starts a server on a fresh data store and runs the compatibility suite against it.
#
#   compat/run.sh slonbase     needs DATABASE_URL (an empty database it may use) and a built
#                              target/debug/slonbase (or SLONBASE_BIN)
#   compat/run.sh pocketbase   needs POCKETBASE_BIN
#
# Extra arguments go to `node --test` (e.g. a test file).
set -euo pipefail
cd "$(dirname "$0")"

target="${1:?usage: run.sh slonbase|pocketbase [node --test args]}"
shift
port="${COMPAT_PORT:-8099}"
export COMPAT_URL="http://127.0.0.1:${port}"
export COMPAT_EMAIL="${COMPAT_EMAIL:-admin@example.com}"
export COMPAT_PASSWORD="${COMPAT_PASSWORD:-admin-password-1}"
work="$(pwd)/.run/${target}"
rm -rf "$work" && mkdir -p "$work"
# the hooks of tests/hooks.test.mjs
cp -r hooks "$work/hooks"

case "$target" in
  slonbase)
    bin="${SLONBASE_BIN:-../target/debug/slonbase}"
    : "${DATABASE_URL:?DATABASE_URL must point at an empty database}"
    common=(--config "$work/slonbase.toml" --storage-path "$work/storage" --backups-path "$work/backups" --hooks-dir "$work/hooks")
    "$bin" "${common[@]}" setup --database-url "$DATABASE_URL" --email "$COMPAT_EMAIL" --password "$COMPAT_PASSWORD" > "$work/setup.log" 2>&1
    "$bin" "${common[@]}" --bind "127.0.0.1:${port}" serve > "$work/server.log" 2>&1 &
    ;;
  pocketbase)
    bin="${POCKETBASE_BIN:?POCKETBASE_BIN must point at a pocketbase binary}"
    "$bin" superuser upsert "$COMPAT_EMAIL" "$COMPAT_PASSWORD" --dir "$work/pb_data" > "$work/setup.log" 2>&1
    "$bin" serve --dir "$work/pb_data" --hooksDir "$work/hooks" --http "127.0.0.1:${port}" > "$work/server.log" 2>&1 &
    ;;
  *)
    echo "unknown target: $target" >&2
    exit 2
    ;;
esac
server=$!
trap 'kill $server 2>/dev/null || true; wait $server 2>/dev/null || true' EXIT

for _ in $(seq 1 100); do
  if curl -sf "${COMPAT_URL}/api/health" > /dev/null; then break; fi
  sleep 0.2
done
if ! curl -sf "${COMPAT_URL}/api/health" > /dev/null; then
  echo "the server didn't start:" >&2
  cat "$work/server.log" >&2
  exit 1
fi

# a new slonbase installation has rate limits on, PocketBase's doesn't: compare like with like
if [ "$target" = slonbase ]; then
  node --input-type=module -e '
    const [url, identity, password] = process.argv.slice(1);
    const send = async (method, path, body, token) => {
      const res = await fetch(url + path, {
        method,
        headers: { "content-type": "application/json", ...(token ? { authorization: token } : {}) },
        body: JSON.stringify(body),
      });
      if (!res.ok) throw new Error(`${method} ${path}: ${res.status} ${await res.text()}`);
      return res.json();
    };
    const { token } = await send("POST", "/api/collections/_superusers/auth-with-password", { identity, password });
    await send("PATCH", "/api/settings", { rateLimits: { enabled: false } }, token);
  ' "$COMPAT_URL" "$COMPAT_EMAIL" "$COMPAT_PASSWORD"
fi

status=0
node --experimental-eventsource --test --test-concurrency=1 "${@:-tests/*.test.mjs}" || status=$?
if [ "$status" -ne 0 ]; then
  echo "--- server log (last 50 lines) ---"
  tail -50 "$work/server.log"
fi
exit "$status"

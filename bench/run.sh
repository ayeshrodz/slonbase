#!/usr/bin/env bash
# Benchmarks one server on the same workloads as the other:
#
#   bench/run.sh slonbase     needs DATABASE_URL (an empty database) and a release build
#                             (target/release/slonbase, or SLONBASE_BIN)
#   bench/run.sh pocketbase   needs POCKETBASE_BIN
#
# Results go to bench/results/<target>.json; bench/report.mjs compares them. The server (and
# PostgreSQL, when PG_PIDS lists its processes) runs on SERVER_CPUS, the load generator (hey)
# on LOAD_CPUS, so they don't compete for the same cores.
set -euo pipefail
cd "$(dirname "$0")"

target="${1:?usage: run.sh slonbase|pocketbase}"
port="${BENCH_PORT:-8098}"
duration="${BENCH_DURATION:-10s}"
warmup="${BENCH_WARMUP:-2s}"
runs="${BENCH_RUNS:-3}"
name="${BENCH_NAME:-$target}"
server_cpus="${SERVER_CPUS:-0-2}"
load_cpus="${LOAD_CPUS:-3}"
hey="${HEY_BIN:-hey}"
export BENCH_URL="http://127.0.0.1:${port}"
export BENCH_EMAIL="bench-admin@example.com"
export BENCH_PASSWORD="bench-admin-password-1"
work="$(pwd)/.run/${target}"
rm -rf "$work" && mkdir -p "$work" results

case "$target" in
  slonbase)
    bin="${SLONBASE_BIN:-../target/release/slonbase}"
    : "${DATABASE_URL:?DATABASE_URL must point at an empty database}"
    common=(--config "$work/slonbase.toml" --storage-path "$work/storage" --backups-path "$work/backups" --hooks-dir "$work/hooks")
    "$bin" "${common[@]}" setup --database-url "$DATABASE_URL" --email "$BENCH_EMAIL" --password "$BENCH_PASSWORD" > "$work/setup.log" 2>&1
    taskset -c "$server_cpus" "$bin" "${common[@]}" --bind "127.0.0.1:${port}" serve > "$work/server.log" 2>&1 &
    version="slonbase $("$bin" --version | awk '{print $2}')"
    ;;
  pocketbase)
    bin="${POCKETBASE_BIN:?POCKETBASE_BIN must point at a pocketbase binary}"
    "$bin" superuser upsert "$BENCH_EMAIL" "$BENCH_PASSWORD" --dir "$work/pb_data" > "$work/setup.log" 2>&1
    taskset -c "$server_cpus" "$bin" serve --dir "$work/pb_data" --http "127.0.0.1:${port}" > "$work/server.log" 2>&1 &
    version="pocketbase $("$bin" --version | awk '{print $3}')"
    ;;
  *)
    echo "unknown target: $target" >&2
    exit 2
    ;;
esac
server=$!
trap 'kill $server 2>/dev/null || true; wait $server 2>/dev/null || true' EXIT
for pid in ${PG_PIDS:-}; do taskset -a -cp "$server_cpus" "$pid" > /dev/null || true; done

for _ in $(seq 1 100); do
  curl -sf "${BENCH_URL}/api/health" > /dev/null && break
  sleep 0.2
done

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
  ' "$BENCH_URL" "$BENCH_EMAIL" "$BENCH_PASSWORD"
fi

echo "seeding ${target}..." >&2
seed="$(node seed.mjs)"
post="$(node -e 'console.log(JSON.parse(process.argv[1]).post)' "$seed")"
token="$(node -e 'console.log(JSON.parse(process.argv[1]).token)' "$seed")"

records="${BENCH_URL}/api/collections/posts/records"
filter='views%20%3E%20500%20%26%26%20published%20%3D%20true'
# name | concurrency | method | url | body | extra header
scenarios=(
  "health|32|GET|${BENCH_URL}/api/health||"
  "view one record|32|GET|${records}/${post}||"
  "view one record (1 client)|1|GET|${records}/${post}||"
  "list 30|32|GET|${records}?perPage=30||"
  "list 30, sorted|32|GET|${records}?perPage=30&sort=-views||"
  "list 30, filtered|32|GET|${records}?perPage=30&filter=${filter}||"
  "list 30, expand author|32|GET|${records}?perPage=30&expand=author||"
  "list 30, signed in|32|GET|${records}?perPage=30||Authorization: ${token}"
  "create record|32|POST|${BENCH_URL}/api/collections/writes/records|{\"title\":\"bench\",\"body\":\"text\",\"views\":5,\"published\":true}|"
  "sign in (bcrypt)|8|POST|${BENCH_URL}/api/collections/bench_users/auth-with-password|{\"identity\":\"bench@example.com\",\"password\":\"bench-password-1\"}|"
)

raw="results/${name}.raw.jsonl"
: > "$raw"
for run in $(seq 1 "$runs"); do
  for s in "${scenarios[@]}"; do
    IFS='|' read -r label conc method url body header <<< "$s"
    args=(-c "$conc" -m "$method" -cpus 1)
    [ -n "$body" ] && args+=(-T "application/json" -d "$body")
    [ -n "$header" ] && args+=(-H "$header")
    echo "  [${run}/${runs}] ${label}" >&2
    taskset -c "$load_cpus" "$hey" -z "$warmup" "${args[@]}" "$url" > /dev/null
    report="$(taskset -c "$load_cpus" "$hey" -z "$duration" "${args[@]}" "$url")"
    rps="$(awk '/Requests\/sec:/ {print $2}' <<< "$report")"
    pct() { awk -v p="$1%" '$1 == p && $2 == "in" {printf "%.2f", $3 * 1000; found = 1} END {if (!found) printf "null"}' <<< "$report"; }
    ok="$(awk '/\[2[0-9][0-9]\]/ {n += $2} END {print n + 0}' <<< "$report")"
    bad="$(awk '/\[[1345][0-9][0-9]\]/ {n += $2} END {print n + 0}' <<< "$report")"
    errors="$(awk '/Error distribution:/ {e = 1; next} e && /\[[0-9]+\]/ {gsub(/[][]/, "", $1); n += $1} END {print n + 0}' <<< "$report")"
    printf '{"name":"%s","concurrency":%s,"rps":%s,"p50":%s,"p90":%s,"p99":%s,"ok":%s,"failed":%s}\n' \
      "$label" "$conc" "${rps:-0}" "$(pct 50)" "$(pct 90)" "$(pct 99)" "$ok" "$((bad + ${errors:-0}))" >> "$raw"
  done
done
node aggregate.mjs "$raw" "$name" "$version" "$duration" "$(nproc)" "${BENCH_NOTE:-}" > "results/${name}.json"
echo "wrote results/${name}.json" >&2

#!/usr/bin/env bash
# Compares slonbase, PocketBase and PostgREST on this machine, in one command, and writes a folder
# ready to publish (bench/published/<slonbase version>/): the three results, their raw runs, the
# report and a description of the machine. See "Running the comparison" in bench/README.md.
#
#   ADMIN_DATABASE_URL='postgres://postgres:...@127.0.0.1:5432/postgres' bench/compare.sh
#
# ADMIN_DATABASE_URL  a role that may create databases (two are made, bench_slonbase and
#                     bench_postgrest, and dropped first if they exist)
# SLONBASE_BIN        a slonbase binary (default: built with cargo build --release)
# POCKETBASE_VERSION  default 0.40.6; checked against the release's checksums.txt
# POSTGREST_VERSION   default 16.5; POSTGREST_SHA256 checks the download (its checksum is
#                     printed either way, to compare with the release page)
# BENCH_RUNS, BENCH_DURATION, SERVER_CPUS, LOAD_CPUS: as in run.sh (3 runs of 10s; the servers
#                     and PostgreSQL on CPUs 0-2, the load on 3)
set -euo pipefail
# (a relative SLONBASE_BIN is read from where the script was started)
[ -n "${SLONBASE_BIN:-}" ] && SLONBASE_BIN="$(realpath "$SLONBASE_BIN")"
cd "$(dirname "$0")"
here="$(pwd)"
: "${ADMIN_DATABASE_URL:?ADMIN_DATABASE_URL must be a PostgreSQL URL of a role that may create databases}"
pb_version="${POCKETBASE_VERSION:-0.40.6}"
pg_version="${POSTGREST_VERSION:-16.5}"
tools="$here/.tools"

say() { printf '\n== %s\n' "$*" >&2; }
fail() { echo "compare.sh: $*" >&2; exit 1; }

say "Checking what's needed"
missing=()
for t in node npm psql curl unzip tar xz sha256sum taskset nproc; do command -v "$t" > /dev/null || missing+=("$t"); done
command -v "${HEY_BIN:-hey}" > /dev/null || missing+=("hey (go install github.com/rakyll/hey@latest)")
[ ${#missing[@]} -eq 0 ] || fail "missing: ${missing[*]}"
[ "$(nproc)" -ge 4 ] || fail "needs at least 4 CPUs (3 for the servers and PostgreSQL, 1 for the load)"
psql "$ADMIN_DATABASE_URL" -qAtc 'select 1' > /dev/null || fail "can't connect with ADMIN_DATABASE_URL"
gov=/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor
if [ -r "$gov" ] && [ "$(cat "$gov")" != performance ]; then
  echo "note: the CPU governor is '$(cat "$gov")'; 'performance' gives steadier numbers (see bench/README.md)" >&2
fi
[ -d node_modules ] || npm ci --no-audit --no-fund > /dev/null

say "slonbase"
if [ -z "${SLONBASE_BIN:-}" ]; then
  (cd .. && cargo build --release --locked)
  SLONBASE_BIN="$(cd .. && pwd)/target/release/slonbase"
fi
version="$("$SLONBASE_BIN" --version | awk '{print $2}')"
echo "slonbase $version: $SLONBASE_BIN" >&2

say "PocketBase $pb_version"
mkdir -p "$tools"
pb_dir="$tools/pocketbase-$pb_version"
if [ ! -x "$pb_dir/pocketbase" ]; then
  base="https://github.com/pocketbase/pocketbase/releases/download/v$pb_version"
  zip="pocketbase_${pb_version}_linux_amd64.zip"
  mkdir -p "$pb_dir" && cd "$pb_dir"
  curl -fsSLO "$base/$zip"
  curl -fsSL "$base/checksums.txt" | grep " $zip\$" | sha256sum -c - >&2 || fail "PocketBase's checksum doesn't match"
  unzip -qo "$zip" pocketbase && rm "$zip"
  cd "$here"
fi
export POCKETBASE_BIN="$pb_dir/pocketbase"

say "PostgREST $pg_version"
pg_dir="$tools/postgrest-$pg_version"
if [ ! -x "$pg_dir/postgrest" ]; then
  asset="${POSTGREST_ASSET:-postgrest-v${pg_version}-linux-static-x86-64.tar.xz}"
  mkdir -p "$pg_dir" && cd "$pg_dir"
  curl -fsSLO "https://github.com/PostgREST/postgrest/releases/download/v$pg_version/$asset"
  sum="$(sha256sum "$asset" | awk '{print $1}')"
  echo "sha256 of $asset: $sum" >&2
  if [ -n "${POSTGREST_SHA256:-}" ] && [ "$sum" != "$POSTGREST_SHA256" ]; then fail "PostgREST's checksum doesn't match POSTGREST_SHA256"; fi
  tar xJf "$asset" && rm "$asset"
  cd "$here"
fi
export POSTGREST_BIN="$pg_dir/postgrest"

say "Databases"
# (the same server and role, another database; a local server without sslmode is reached without
# TLS, as its certificate is usually self-signed)
db_url() {
  node -e '
    const u = new URL(process.argv[1]);
    u.pathname = "/" + process.argv[2];
    if (!u.searchParams.has("sslmode") && ["localhost", "127.0.0.1", "[::1]", ""].includes(u.hostname)) u.searchParams.set("sslmode", "disable");
    console.log(u.href);
  ' "$ADMIN_DATABASE_URL" "$1"
}
for db in bench_slonbase bench_postgrest; do
  psql "$ADMIN_DATABASE_URL" -qc "DROP DATABASE IF EXISTS $db WITH (FORCE)" -c "CREATE DATABASE $db" > /dev/null
done
# PostgreSQL runs on the servers' CPUs too, when it is on this machine
PG_PIDS="$(pgrep -d ' ' -x postgres || true)"
export PG_PIDS SLONBASE_BIN

say "Running (3 servers, about $(( ${BENCH_RUNS:-3} * 10 * 11 * 3 / 60 + 3 )) minutes)"
rm -rf results
# a server that fails says why: the end of its logs
run() {
  local target="$1"; shift
  if ! env "$@" ./run.sh "$target"; then
    tail -n 20 ".run/$target"/*.log >&2 || true
    fail "the $target run failed"
  fi
}
run slonbase DATABASE_URL="$(db_url bench_slonbase)"
run pocketbase
run postgrest DATABASE_URL="$(db_url bench_postgrest)"

say "Publishing to bench/published/$version"
out="published/$version"
mkdir -p "$out"
for n in slonbase pocketbase postgrest; do cp "results/$n.json" "results/$n.raw.jsonl" "$out/"; done
{
  echo "$(lscpu | sed -n 's/^Model name:[[:space:]]*//p' | head -1), $(nproc) CPUs, $(awk '/MemTotal/ {printf "%.0f GB", $2 / 1048576}' /proc/meminfo)"
  echo "$( . /etc/os-release && echo "$PRETTY_NAME"), Linux $(uname -r)"
  echo "PostgreSQL $(psql "$ADMIN_DATABASE_URL" -qAtc 'show server_version')"
  [ -r "$gov" ] && echo "CPU governor: $(cat "$gov")"
  echo "servers and PostgreSQL on CPUs ${SERVER_CPUS:-0-2}, the load generator on ${LOAD_CPUS:-3}"
  echo "${BENCH_RUNS:-3} runs of ${BENCH_DURATION:-10s} per scenario, after a ${BENCH_WARMUP:-2s} warm-up"
} > "$out/machine.txt"
node report.mjs "$out" > "$out/REPORT.md"
cat "$out/REPORT.md"
cat >&2 <<EOF

Done: bench/published/$version
Send that folder back (or commit it on a branch): with it, node scripts/bench-figures.mjs
updates the tables in README.md and docs/benchmarks.md.
EOF

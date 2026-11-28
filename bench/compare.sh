#!/usr/bin/env bash
# Compares slonbase, PocketBase and PostgREST on this machine, in one command, and writes a folder
# ready to publish (bench/published/<slonbase version>/): the three results, their raw runs, the
# report and a description of the machine. See "Running the comparison" in bench/README.md.
#
#   ADMIN_DATABASE_URL='postgres://postgres:...@127.0.0.1:5432/postgres' bench/compare.sh
#
# ADMIN_DATABASE_URL  a role that may create databases (two are made, bench_slonbase and
#                     bench_postgrest, and dropped first if they exist) and may use the role
#                     PostgREST answers as, anon (or create it: CREATEROLE)
# SLONBASE_BIN        a slonbase binary (default: built with cargo build --release)
# POCKETBASE_VERSION  default 0.40.6; checked against the release's checksums.txt
# POSTGREST_VERSION   default 16.5; POSTGREST_SHA256 checks the download (its checksum is
#                     printed either way, to compare with the release page)
# BENCH_REPLACE       1 replaces an existing bench/published/<version>/ (otherwise the run stops)
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
# what the role may do: make the two databases, and answer PostgREST's requests as anon
IFS='|' read -r db_role can_super can_db can_role anon_ok < <(psql "$ADMIN_DATABASE_URL" -qAtc "
  select current_user, rolsuper, rolcreatedb, rolcreaterole,
         coalesce((select pg_has_role(current_user, 'anon', 'MEMBER') from pg_roles where rolname = 'anon'), false)
  from pg_roles where rolname = current_user")
as_postgres="sudo -u postgres psql -c"
if [ "$can_super" != t ] && [ "$can_db" != t ]; then
  fail "the role $db_role can't create databases: $as_postgres 'ALTER ROLE \"$db_role\" CREATEDB'"
fi
if [ "$can_super" != t ] && [ "$anon_ok" != t ] && [ "$can_role" != t ]; then
  fail "PostgREST answers as the role anon, which $db_role can't use or create: $as_postgres 'CREATE ROLE anon NOLOGIN' -c 'GRANT anon TO \"$db_role\"' (or give $db_role CREATEROLE)"
fi
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
# a published folder holds the results of one comparison, so a folder already there (from an
# earlier run, or results published by hand) is replaced only when BENCH_REPLACE=1 says so
if [ -e "published/$version" ] && [ "${BENCH_REPLACE:-}" != 1 ]; then
  fail "bench/published/$version already exists; move it away, or set BENCH_REPLACE=1 to replace it"
fi

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
# PostgreSQL runs on the servers' CPUs too, when it is on this machine. Pinning its processes
# needs root unless they are this user's, so it is checked here: new connections inherit the
# main process's CPUs, so pinning that one (with sudo, once) is enough
PG_PIDS="$(pgrep -d ' ' -x postgres || true)"
pinned=""
if [ -n "$PG_PIDS" ]; then
  server_cpus="${SERVER_CPUS:-0-2}"
  for pid in $PG_PIDS; do taskset -a -cp "$server_cpus" "$pid" > /dev/null 2>&1 || true; done
  want="$(taskset -c "$server_cpus" sh -c 'taskset -cp $$' | sed 's/.*: //')"
  main_pid="$(pgrep -o -x postgres)"
  have="$(taskset -cp "$main_pid" | sed 's/.*: //')"
  if [ "$have" != "$want" ]; then
    fail "PostgreSQL runs on CPUs $have, not the servers' $want, and this user can't move it. Pin it once (connections made later follow):
  for p in \$(pgrep -x postgres); do sudo taskset -a -cp $server_cpus \"\$p\" > /dev/null; done
and run this again; a PostgreSQL restart undoes it."
  fi
  pinned=1
fi
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
# (the folder is removed only when asked; a plain mkdir fails on one that appeared while the
# servers ran, so another run's results are never deleted)
if [ "${BENCH_REPLACE:-}" = 1 ]; then rm -rf "$out"; fi
mkdir -p published
mkdir "$out" 2> /dev/null || fail "bench/published/$version already exists; move it away, or set BENCH_REPLACE=1 to replace it"
for n in slonbase pocketbase postgrest; do cp "results/$n.json" "results/$n.raw.jsonl" "$out/"; done
{
  echo "$(lscpu | sed -n 's/^Model name:[[:space:]]*//p' | head -1), $(nproc) CPUs, $(awk '/MemTotal/ {printf "%.0f GB", $2 / 1048576}' /proc/meminfo)"
  echo "$( . /etc/os-release && echo "$PRETTY_NAME"), Linux $(uname -r)"
  echo "PostgreSQL $(psql "$ADMIN_DATABASE_URL" -qAtc 'show server_version')"
  [ -r "$gov" ] && echo "CPU governor: $(cat "$gov")"
  turbo=/sys/devices/system/cpu/intel_pstate/no_turbo boost=/sys/devices/system/cpu/cpufreq/boost
  if [ -r "$turbo" ]; then echo "turbo boost: $([ "$(cat "$turbo")" = 1 ] && echo off || echo on)"
  elif [ -r "$boost" ]; then echo "turbo boost: $([ "$(cat "$boost")" = 0 ] && echo off || echo on)"; fi
  if [ -n "$pinned" ]; then
    echo "servers and PostgreSQL on CPUs ${SERVER_CPUS:-0-2}, the load generator on ${LOAD_CPUS:-3}"
  else
    echo "servers on CPUs ${SERVER_CPUS:-0-2}, the load generator on ${LOAD_CPUS:-3}; PostgreSQL on another machine"
  fi
  echo "${BENCH_RUNS:-3} runs of ${BENCH_DURATION:-10s} per scenario, after a ${BENCH_WARMUP:-2s} warm-up"
} > "$out/machine.txt"
node report.mjs "$out" > "$out/REPORT.md"
cat "$out/REPORT.md"
cat >&2 <<EOF

Done: bench/published/$version
Send that folder back (or commit it on a branch): with it, node scripts/bench-figures.mjs
updates the tables and figures in the README, the docs and the project page.
EOF

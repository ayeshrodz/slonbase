#!/usr/bin/env bash
# Runs several slonbase instances behind a balancer with no affinity, on one PostgreSQL, and asks
# them the questions in scenarios.mjs (tokens, schema and settings, rate limits, realtime, projects).
#
#   FLEET_ADMIN_URL='postgres://postgres:postgres@127.0.0.1:5432/postgres?sslmode=disable' \
#     scripts/fleet-test/run.sh [instances]          # default 3
#   FLEET_POOLER=pgbouncer scripts/fleet-test/run.sh    # the same, with PgBouncer in transaction mode
#
# Needs: a slonbase binary (SLONBASE_BIN, default target/debug/slonbase), a PostgreSQL role that
# may create roles and databases (FLEET_ADMIN_URL), Node 22, psql, and for FLEET_POOLER=pgbouncer
# the pgbouncer binary. The instances listen on 8101..., the balancer on 9000, PgBouncer on 6439 (FLEET_PGB_PORT).
set -euo pipefail
cd "$(dirname "$0")"
bin="$(realpath "${SLONBASE_BIN:-../../target/debug/slonbase}")"
admin="${FLEET_ADMIN_URL:-postgres://postgres:postgres@127.0.0.1:5432/postgres?sslmode=disable}"
n="${1:-3}"
pooler="${FLEET_POOLER:-none}"
pgb_port="${FLEET_PGB_PORT:-6439}"
work="$(mktemp -d)"
chmod 777 "$work"   # PgBouncer may run as another user and writes its log and pid here
pids=()
cleanup() {
  for p in "${pids[@]:-}"; do kill "$p" 2>/dev/null || true; done
  [ -f "$work/pgbouncer.pid" ] && kill "$(cat "$work/pgbouncer.pid")" 2>/dev/null || true
  for d in $(psql "$admin" -qtAc "select datname from pg_database where datname like 'fleet\_%'"); do psql "$admin" -qc "DROP DATABASE IF EXISTS \"$d\" WITH (FORCE)" >/dev/null 2>&1 || true; done
  psql "$admin" -qc "DROP ROLE IF EXISTS fleet" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

host_port="$(psql "$admin" -qtAc "select inet_server_port()")"
psql "$admin" -qc "DROP ROLE IF EXISTS fleet" >/dev/null 2>&1 || true
psql "$admin" -qc "CREATE ROLE fleet LOGIN PASSWORD 'fleet' CREATEDB CREATEROLE" >/dev/null
psql "$admin" -qc "DROP DATABASE IF EXISTS fleet_main WITH (FORCE)" -c "CREATE DATABASE fleet_main OWNER fleet" >/dev/null 2>&1
direct_url="postgres://fleet:fleet@127.0.0.1:${host_port}/fleet_main?sslmode=disable"
url="$direct_url"
export FLEET_DATABASE_URL="$direct_url"

if [ "$pooler" = pgbouncer ]; then
  printf '"fleet" "fleet"\n' > "$work/userlist.txt"
  cat > "$work/pgbouncer.ini" <<INI
[databases]
* = host=127.0.0.1 port=${host_port}
[pgbouncer]
listen_addr = 127.0.0.1
listen_port = ${pgb_port}
auth_type = scram-sha-256
auth_file = $work/userlist.txt
pool_mode = transaction
max_client_conn = 3000
default_pool_size = 10
max_prepared_statements = 300
ignore_startup_parameters = extra_float_digits,options
logfile = $work/pgbouncer.log
pidfile = $work/pgbouncer.pid
INI
  chmod 644 "$work"/*
  if [ "$(id -u)" = 0 ]; then runuser -u postgres -- pgbouncer -d "$work/pgbouncer.ini"; else pgbouncer -d "$work/pgbouncer.ini"; fi
  for _ in $(seq 1 50); do (exec 3<>"/dev/tcp/127.0.0.1/${pgb_port}") 2>/dev/null && break; sleep 0.2; done
  if ! (exec 3<>"/dev/tcp/127.0.0.1/${pgb_port}") 2>/dev/null; then echo "PgBouncer did not start on port ${pgb_port}:"; cat "$work/pgbouncer.log" 2>/dev/null; exit 1; fi
  url="postgres://fleet:fleet@127.0.0.1:${pgb_port}/fleet_main?sslmode=disable"
  export SLONBASE_DATABASE_LISTEN_URL="$direct_url"   # LISTEN needs a direct connection
  echo "through PgBouncer (transaction mode); notifications on a direct connection"
fi
export SLONBASE_DATABASE_URL="$url"

"$bin" --config "$work/setup.toml" --storage-path "$work/s0" setup --email admin@example.com --password admin-password-1 > "$work/setup.log" 2>&1 || {
  echo "slonbase setup failed:"; sed 's/\x1b\[[0-9;]*m//g' "$work/setup.log" | tail -n 20
  [ -f "$work/pgbouncer.log" ] && { echo "PgBouncer log:"; tail -n 20 "$work/pgbouncer.log"; }
  exit 1
}
ports=()
for i in $(seq 1 "$n"); do
  d="$work/i$i"; mkdir -p "$d"
  "$bin" --config "$d/config.toml" --storage-path "$d/storage" --backups-path "$d/backups" --hooks-dir "$d/hooks" \
    --projects-path "$d/projects" --bind "127.0.0.1:$((8100 + i))" serve > "$d/log" 2>&1 &
  pids+=("$!"); ports+=("$((8100 + i))")
done
for p in "${ports[@]}"; do for _ in $(seq 1 150); do curl -sf "127.0.0.1:$p/api/health" > /dev/null && break; sleep 0.2; done; done
node lb.mjs 9000 "${ports[@]}" > "$work/lb.log" 2>&1 &
pids+=("$!")
sleep 1
echo "$n instances behind the balancer"
set +e
node scenarios.mjs 9000 "${ports[@]}"
status=$?
if [ $status -ne 0 ]; then tail -n 20 "$work"/i1/log | sed 's/\x1b\[[0-9;]*m//g'; fi
exit $status

#!/usr/bin/env bash
# Builds the demo installation, then the dashboard screenshots and the tour video used by the
# README and the site:
#
#   DATABASE_URL='postgres://...an empty database with CREATEDB and CREATEROLE...' scripts/demo/run.sh
#
# SLONBASE_BIN (default target/debug/slonbase), DEMO_PORT (8222), OUT (default scripts/demo/out).
# Needs Node 22, `npm ci` in ui/test (Playwright's Chromium) and ffmpeg.
set -euo pipefail
cd "$(dirname "$0")"
: "${DATABASE_URL:?DATABASE_URL must point at an empty database}"
bin="$(realpath "${SLONBASE_BIN:-../../target/debug/slonbase}")"
port="${DEMO_PORT:-8222}"
out="$(realpath -m "${OUT:-out}")"
work="$out/.run"
rm -rf "$work" && mkdir -p "$work"
common=(--config "$work/slonbase.toml" --storage-path "$work/storage" --backups-path "$work/backups" --hooks-dir "$work/hooks" --projects-path "$work/projects")
export SLONBASE_SQL_CONSOLE=true
"$bin" "${common[@]}" setup --database-url "$DATABASE_URL" --email admin@slonbase.dev --password admin-password-1 > "$work/setup.log" 2>&1
"$bin" "${common[@]}" --bind "127.0.0.1:$port" serve > "$work/server.log" 2>&1 &
server=$!
trap 'kill $server 2>/dev/null || true; wait $server 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do curl -sf "http://127.0.0.1:$port/api/health" > /dev/null && break; sleep 0.2; done
for p in analytics shop blog; do
  SLONBASE_SUPERUSER_PASSWORD=admin-password-1 "$bin" "${common[@]}" project create "$p" --email admin@slonbase.dev > /dev/null
done
base="http://127.0.0.1:$port"
node seed.mjs "$base"
node traffic.mjs "$base"
curl -s -X POST "$base/api/backups" -H "authorization: $(curl -s -X POST "$base/api/collections/_superusers/auth-with-password" -H 'content-type: application/json' -d '{"identity":"admin@slonbase.dev","password":"admin-password-1"}' | node -pe 'JSON.parse(require("fs").readFileSync(0)).token')" -H 'content-type: application/json' -d '{"name":"demo.zip"}' > /dev/null

[ -n "${SKIP_SCREENSHOTS:-}" ] || node screenshots.mjs "$base" "$out/screenshots"

# the tour: the server and data are reset first so the video always starts the same way
node tour.mjs "$base" "$out/tour"
raw="$(ls -t "$out"/tour/*.webm | head -1)"
ffmpeg -y -loglevel error -i "$raw" -vf "fps=30,scale=1280:800:flags=lanczos" -c:v libx264 -preset slow -crf 24 -pix_fmt yuv420p -movflags +faststart -an "$out/tour.mp4"
ffmpeg -y -loglevel error -i "$raw" -vf "fps=30,scale=1280:800:flags=lanczos" -c:v libvpx-vp9 -b:v 0 -crf 36 -row-mt 1 -pix_fmt yuv420p -an "$out/tour.webm"
ffmpeg -y -loglevel error -ss 3 -i "$out/tour.mp4" -frames:v 1 -q:v 3 "$out/tour-poster.jpg"
ls -la "$out"/tour.* "$out/tour-poster.jpg"

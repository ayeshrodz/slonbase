#!/usr/bin/env bash
# Assembles the site as the deploy does (.github/workflows/repo.yml: the project page, the
# assets, the fonts and the documentation) and runs the checks in check.mjs on it:
#
#   site/test/run.sh
#
# Needs `npm ci` here and Playwright's Chromium (`npx playwright install chromium`), or
# CHROMIUM_PATH pointing at a Chromium.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
site="$(mktemp -d)"
trap 'rm -rf "$site"' EXIT

cp -r "$repo/site/." "$site/"
rm -rf "$site/test"
mkdir -p "$site/assets/fonts"
cp -r "$repo/docs/assets/." "$site/assets/"
cp "$repo/ui/app/fonts/"* "$site/assets/fonts/"
node "$repo/scripts/build-docs.mjs" "$site/docs" >/dev/null
# the release regions as the deploy fills them (offline: from Cargo.toml and CHANGELOG.md)
node "$repo/scripts/build-site.mjs" "$site/index.html" --offline >/dev/null

node "$here/check.mjs" "$site"

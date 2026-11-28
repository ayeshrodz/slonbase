#!/usr/bin/env bash
# Decides how much of CI a change needs, so a documentation fix doesn't wait for the whole test
# matrix (.github/workflows/ci.yml, job `plan`). The changed paths come on stdin, one per line;
# nothing on stdin means the paths are unknown. The answer is on stdout, as `key=value` lines for
# $GITHUB_OUTPUT:
#
#   code      the Rust side runs: build, tests, compatibility, installer, upgrade, dashboard
#   web       the project page's and the documentation's checks run
#   full      everything runs, on every supported PostgreSQL: a release, a night, a manual run, or
#             a change to what the tests are built from
#   postgres  the PostgreSQL versions the tests run on, as JSON
#   reason    why, in a line
#
# CI_EVENT is the workflow's event (pull_request, push, schedule, workflow_dispatch). CI_RELEASE
# is 1 when the commit has a version that has no tag yet, which the Release workflow publishes.
#
# A path counts as code unless it is listed as not being (the lists are the default's opposite on
# purpose: a file nobody thought of is tested). scripts/ci-plan.sh --test checks the rules.
set -euo pipefail

# paths whose change needs no build and no test of the product
light() {
  case "$1" in
    site/install.sh) return 1 ;;   # the installer's own tests run on it
    docs/* | site/* | bench/* | scripts/docs/* | scripts/demo/*) return 0 ;;
    scripts/build-docs.mjs | scripts/build-site.mjs | scripts/build-tokens.mjs | scripts/bench-figures.mjs | scripts/social-preview.html) return 0 ;;
    LICENSE | NOTICE | .gitignore) return 0 ;;
    .github/ISSUE_TEMPLATE/* | .github/CODEOWNERS | .github/dependabot.yml | .github/pull_request_template.md) return 0 ;;
    .github/workflows/ci.yml) return 1 ;;
    .github/workflows/*) return 0 ;;  # checked by actionlint in `lint`, run by their own events
    */*) return 1 ;;
    *.md) return 0 ;;                 # README.md, CHANGELOG.md, ... in the root
    *) return 1 ;;
  esac
}

# paths the page's and the documentation's checks read
web() {
  case "$1" in
    site/install.sh) return 1 ;;
    docs/* | site/* | scripts/docs/* | scripts/build-docs.mjs | scripts/build-site.mjs | scripts/build-tokens.mjs | scripts/bench-figures.mjs | ui/app/fonts/* | ui/app/app.css) return 0 ;;
    *) return 1 ;;
  esac
}

# paths that change what every test, or every PostgreSQL, sees: the whole matrix runs
wide() {
  case "$1" in
    .github/workflows/ci.yml | .github/actions/* | scripts/ci-plan.sh | .config/nextest.toml) return 0 ;;
    Cargo.toml | Cargo.lock | rust-toolchain | rust-toolchain.toml | migrations/*) return 0 ;;
    *) return 1 ;;
  esac
}

plan() {
  local event="${CI_EVENT:-pull_request}" release="${CI_RELEASE:-0}" files path
  local full=false code=false web=false reason=""
  files="$(cat)"
  files="$(printf '%s\n' "$files" | sed '/^$/d')"

  if [ -z "$files" ]; then
    full=true; reason="the changed files are unknown"
  else
    while IFS= read -r path; do
      light "$path" || code=true
      web "$path" && web=true
      if wide "$path" && [ "$full" = false ]; then full=true; reason="$path changed"; fi
    done <<< "$files"
  fi
  case "$event" in
    schedule) full=true; reason="the nightly run" ;;
    workflow_dispatch) full=true; reason="started by hand" ;;
  esac
  if [ "$release" = 1 ]; then full=true; reason="a release: the version in Cargo.toml has no tag yet"; fi

  local postgres='["14","18"]'
  if [ "$full" = true ]; then
    code=true; web=true
    postgres='["14","15","16","17","18"]'
  fi
  if [ -z "$reason" ]; then
    if [ "$code" = true ]; then reason="code changed: the oldest and the newest PostgreSQL"
    else reason="only documentation, the page, benchmarks or workflows changed"; fi
  fi
  printf 'full=%s\ncode=%s\nweb=%s\npostgres=%s\nreason=%s\n' "$full" "$code" "$web" "$postgres" "$reason"
}

if [ "${1:-}" = --test ]; then
  failed=0
  check() { # name, event, release, expected "full code web postgres", paths...
    local name="$1" event="$2" release="$3" want="$4" got; shift 4
    got="$(printf '%s\n' "$@" | CI_EVENT="$event" CI_RELEASE="$release" "$0" | sed -n 's/^\(full\|code\|web\|postgres\)=//p' | tr '\n' ' ' | sed 's/ $//')"
    if [ "$got" = "$want" ]; then echo "ok   $name"; else echo "FAIL $name: wanted [$want], got [$got]"; failed=1; fi
  }
  two='["14","18"]'; all='["14","15","16","17","18"]'
  check "a documentation page" pull_request 0 "false false true $two" docs/api.md
  check "the page and its tests" pull_request 0 "false false true $two" site/index.html site/test/check.mjs
  check "a benchmark result" pull_request 0 "false false false $two" bench/published/2.7.1/slonbase.json README.md
  check "screenshots and the media workflow" pull_request 0 "false false true $two" docs/assets/screenshots/a.png .github/workflows/media.yml scripts/demo/tour.mjs
  check "another workflow" pull_request 0 "false false false $two" .github/workflows/release.yml
  check "Rust code" pull_request 0 "false true false $two" src/jobs.rs
  check "a test" pull_request 0 "false true false $two" tests/operations.rs
  check "the dashboard" pull_request 0 "false true false $two" ui/app/pages/data.js
  check "the installer" pull_request 0 "false true false $two" site/install.sh
  check "code and the page together" pull_request 0 "false true true $two" src/jobs.rs docs/api.md
  check "an unlisted file is code" pull_request 0 "false true false $two" something/new.txt
  check "a root file that isn't markdown" pull_request 0 "false true false $two" Dockerfile
  check "the pull request template" pull_request 0 "false false false $two" .github/pull_request_template.md .gitignore MAINTAINING.md
  check "a nested markdown file in code" pull_request 0 "false true false $two" src/NOTES.md
  check "CI itself" pull_request 0 "true true true $all" .github/workflows/ci.yml
  check "the plan" pull_request 0 "true true true $all" scripts/ci-plan.sh docs/api.md
  check "a dependency" pull_request 0 "true true true $all" Cargo.lock
  check "a migration" pull_request 0 "true true true $all" migrations/0026_x.sql
  check "unknown files" pull_request 0 "true true true $all"
  check "a release" push 1 "true true true $all" docs/api.md
  check "the night" schedule 0 "true true true $all" docs/api.md
  check "by hand" workflow_dispatch 0 "true true true $all" docs/api.md
  check "a push to main" push 0 "false false true $two" docs/api.md
  exit "$failed"
fi

plan

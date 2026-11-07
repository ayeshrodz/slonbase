# Contributing to slonbase

Thanks for your interest! Bug reports, fixes, documentation and compatibility reports are all
welcome.

## Reporting bugs

Open an issue with the slonbase version (`slonbase --version`), the PostgreSQL version, how you
run it, and the steps to reproduce. For a difference from PocketBase's behavior, say what
PocketBase does. A failing test in [`compat/`](compat/) is the best report there is.

Security problems go through [SECURITY.md](SECURITY.md), not public issues.

## Development setup

You need Rust 1.87 or newer,
and for most tests a PostgreSQL server where a role can create databases:

```sh
cargo build
cargo test                                   # unit tests; integration tests skip without a database
export SLONBASE_TEST_DATABASE_URL='postgres://postgres:postgres@127.0.0.1:5432/postgres?sslmode=disable'
cargo test                                   # everything: each test creates and drops its own database
```

The S3 tests also need an S3-compatible server (MinIO works):
`SLONBASE_TEST_S3_ENDPOINT=http://127.0.0.1:9000 SLONBASE_TEST_S3_ACCESS_KEY=... SLONBASE_TEST_S3_SECRET=... cargo test`.

The compatibility suite runs the official PocketBase SDK against a running server, and is
expected to pass against both slonbase and PocketBase:

```sh
(cd compat && npm ci)
cargo build && DATABASE_URL='postgres://...an empty database...' compat/run.sh slonbase
POCKETBASE_BIN=/path/to/pocketbase compat/run.sh pocketbase
```

The dashboard is plain ES modules in [`ui/`](ui/), embedded into the binary at build time, with no
build step. See [ui/README.md](ui/README.md).

## Pull requests

- Keep them focused: one change per pull request, with tests.
- Run what CI runs:

  ```sh
  cargo fmt --all --check
  cargo clippy --all-targets --locked -- -D warnings
  cargo test
  cargo audit        # known vulnerabilities in dependencies (cargo install cargo-audit)
  ```

- The API stays compatible with the PocketBase SDKs. When a change touches behavior the SDKs see,
  add a test to `compat/` for it, and run that test against PocketBase too.
- Update the docs (`docs/`, `README.md`) and add a line to `CHANGELOG.md` under
  "Unreleased" for user-visible changes.
- Write commit messages that say what changed and why.

Please follow the [code of conduct](CODE_OF_CONDUCT.md).

slonbase is licensed under the [Apache License 2.0](LICENSE). Before your first pull request is
merged, you sign the [contributor license agreement](CLA.md): a bot comments on the pull request,
and you reply with the sentence it shows. You keep your copyright; the agreement licenses your
work to the project under Apache 2.0 and lets the maintainer license it under other terms in
future (the project may one day offer separately licensed commercial features or hosted services).
See [license and contributing terms](docs/licensing.md).

## Releases and maintaining

Releases, the package signing key, GitHub Pages and the domain are the maintainer's to run: see
[MAINTAINING.md](MAINTAINING.md). A pull request never needs any of it.

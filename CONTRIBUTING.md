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

By contributing, you agree that your contributions are licensed under the [MIT license](LICENSE).

## Releases

Maintainers release by:

1. moving the "Unreleased" entries in `CHANGELOG.md` into a section for the new version;
2. setting the version in `Cargo.toml`;
3. pushing a `vX.Y.Z` tag.

The release workflow checks that the tag matches `Cargo.toml` and that the changelog has the
version's section. It then builds the packages and publishes the GitHub release, with those notes,
and the Docker image. Last, it starts the **Package repositories** workflow, which rebuilds the
apt and dnf repositories from every release's packages and publishes them on GitHub Pages,
with the project's page (`site/index.html`, using the images in `docs/assets/`) at the root; it
also runs when the page changes on `main`.

The repositories need, once:

1. a signing key without a passphrase, kept in the `REPO_GPG_PRIVATE_KEY` secret:

   ```sh
   gpg --batch --passphrase '' --quick-gen-key "slonbase packages <you@example.com>" rsa4096 sign never
   gpg --armor --export-secret-keys "slonbase packages" | gh secret set REPO_GPG_PRIVATE_KEY
   ```

   Keep a copy of the key somewhere safe. A new key means every user has to download the
   public key again.
2. Settings → Pages → Source: **GitHub Actions**.

The workflow can also be run by hand (Actions → Package repositories → Run workflow), for
example after editing a release.

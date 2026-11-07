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

## Releases

Maintainers release by:

1. moving the "Unreleased" entries in `CHANGELOG.md` into a section for the new version;
2. setting the version in `Cargo.toml`;
3. pushing a `vX.Y.Z` tag.

Bumping the version on `main` is also a release: when CI passes on `main` and `Cargo.toml` has a
version without a tag, the **Release** workflow tags that commit and releases it. It checks that
the changelog has the version's section, builds the packages once (amd64 and arm64), publishes
the GitHub release, with those notes, and the Docker image (built from the packages' binaries).
Last, it starts the **Package repositories** workflow, which rebuilds the apt and dnf
repositories from the five newest releases' packages (the Pages artifact is limited to 1 GB;
older packages stay on their release pages) and publishes them on GitHub Pages at
<https://slonbase.com>, with the project's page (`site/index.html` and `site/install.sh`, using
the images in `docs/assets/`) at the root and the documentation site at `/docs/`. It also runs
when the page or the docs change on `main`. CI doesn't build packages or the source image on
every push: the Release does, before it publishes anything, and the **Docker** workflow builds
the image from source only when its inputs change.

The screenshots in `docs/assets/screenshots/` and the tour video on the page come from
`scripts/demo/run.sh` (it seeds a demo installation, takes the screenshots and records and encodes
the tour; see the script's header for what it needs). The benchmark behind the page's numbers is in
`bench/`.

The documentation site is built from `docs/*.md` and `CHANGELOG.md` by
`scripts/build-docs.mjs` (Node, no dependencies). To preview it, build it next to the page and
serve the folder:

```sh
mkdir -p /tmp/site && cp -r site/. /tmp/site && cp -r docs/assets /tmp/site/assets
node scripts/build-docs.mjs /tmp/site/docs && python3 -m http.server -d /tmp/site 8000
```

The build fails on a link to a page or heading that doesn't exist (CI runs it). A new guide goes
in `SECTIONS` at the top of the script.

The repositories need, once:

1. a signing key without a passphrase, kept in the `REPO_GPG_PRIVATE_KEY` secret:

   ```sh
   gpg --batch --passphrase '' --quick-gen-key "slonbase packages <you@example.com>" rsa4096 sign never
   gpg --armor --export-secret-keys "slonbase packages" | gh secret set REPO_GPG_PRIVATE_KEY
   ```

   Keep a copy of the key somewhere safe. A new key means every user has to download the
   public key again.
2. Settings → Pages → Source: **GitHub Actions**.
3. The custom domain, once: Settings → Pages → Custom domain `slonbase.com`, then **Enforce
   HTTPS** when the certificate is ready. At the registrar (Namecheap, with its default
   *BasicDNS* nameservers, under Advanced DNS) delete the parking records, then add `A` records
   for host `@` to `185.199.108.153`, `185.199.109.153`, `185.199.110.153` and
   `185.199.111.153`, optionally `AAAA` records to `2606:50c0:8000::153` to `8003::153`, and
   a `CNAME` for host `www` to `ayeshrodz.github.io.` (a `www` that fails GitHub's check is
   usually a missing record, or custom nameservers). To keep the domain from being claimed by
   someone else, also verify it in the account settings (Pages → Add a domain). The address the
   `.list` and `.repo` files carry comes from the repository variable `SITE_URL` (default
   `https://slonbase.com`), which a fork sets to its own.

The workflow can also be run by hand (Actions → Package repositories → Run workflow), for
example after editing a release.

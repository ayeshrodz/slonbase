# Maintaining slonbase

For the maintainer. A contributor doesn't need anything on this page (see
[CONTRIBUTING.md](CONTRIBUTING.md)).

## How changes reach `main`

Every change, the maintainer's and an agent's included, arrives by pull request and merges only
when CI passes (squash merge, branch deleted afterwards). `main` is protected by a ruleset:

- pull request required, with the CI checks passing and conversations resolved;
- no force pushes, no deletion, linear history;
- code owner review for the paths in `.github/CODEOWNERS` once there is a second maintainer
  (while the maintainer is the only one, the required number of approving reviews is 0, because
  nobody could approve their own pull request);
- CI runs on pull requests and on `main`, not on every branch push (a branch with an open pull
  request would otherwise run twice, and the cancelled duplicate counts as a failed required
  check). To test a branch without a pull request, such as a backport before its tag, run CI by
  hand: Actions, CI, Run workflow, on that branch;
- release tags (`v*`) can't be deleted or moved (creating them stays open, because the Release
  workflow creates the tag with the workflow token, which a ruleset can't exempt).

The steps for setting this up on GitHub are in the repository's settings guide (the PDF that
came with this change, and the checklist at the end of this page).

## Releasing

Maintainers release by:

1. moving the "Unreleased" entries in `CHANGELOG.md` into a section for the new version;
2. setting the version in `Cargo.toml`;
3. pushing a `vX.Y.Z` tag.

Bumping the version on `main` is also a release: when CI passes on `main` and `Cargo.toml` has a
version without a tag, the **Release** workflow tags that commit and releases it. It checks that
the changelog has the version's section, builds the packages once (amd64 and arm64), publishes
the GitHub release, with those notes, and the Docker image (built from the packages' binaries).
With the packages it publishes an SBOM (`scripts/sbom.py`: SPDX, from `Cargo.lock` and the
dashboard's vendored files) and attests every file and the image (build provenance, and the
SBOM for the archives and packages; `gh attestation verify` checks them). When a vendored file
in `ui/app/vendor/` or a font changes, update its entry in `scripts/sbom.py`; CI fails until
then. Last, it starts the **Package repositories** workflow, which rebuilds the apt and dnf
repositories from the packages of the supported releases: the newest three versions of each
line in `packaging/supported-lines.txt`, and always the newest release (the Pages artifact is
limited to 1 GB; older packages stay on their release pages) and publishes them on GitHub Pages at
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

1. a signing key without a passphrase, kept as the `REPO_GPG_PRIVATE_KEY` secret of the
   `package-signing` environment (Settings → Environments), not as a repository secret: only the
   workflow's `sign` job runs in that environment, so the page and docs build never see the key.
   Limit the environment's deployment branches to `main`.

   ```sh
   gpg --batch --passphrase '' --quick-gen-key "slonbase packages <you@example.com>" rsa4096 sign never
   gpg --armor --export-secret-keys "slonbase packages" | gh secret set REPO_GPG_PRIVATE_KEY --env package-signing
   ```

   Keep a copy of the key somewhere safe. A new key means every user has to download the
   public key again. The workflow refuses any key but the one users trust (fingerprint
   `95697FA73AFBAC43E64962F1C7DF885C74BB3942`, published at https://slonbase.com/slonbase.asc);
   after a deliberate key change, or in a fork, set the repository variable
   `REPO_KEY_FINGERPRINT` to the new fingerprint.
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

## Workflow actions

Every action in `.github/workflows/` is pinned to a full commit SHA with its version in a
comment (`uses: actions/checkout@<sha> # v7.0.1`): a tag can be moved, a commit can't. Dependabot
(weekly, grouped) updates the pin and the comment together. When you add an action, pin it the
same way (`git ls-remote https://github.com/<owner>/<repo> refs/tags/<version>` gives the
commit). `dtolnay/rust-toolchain` is pinned to its `stable` branch's commit; the Rust version
itself still comes from the `stable` channel when the job runs.

## When a release fails halfway

The release publishes in order: GitHub release and tag, then the container image, then the
package repositories. If the image fails after the tag exists, re-running the workflow doesn't
skip anything by itself, because the tag is the marker. To resume:

1. Open the failed **Release** run, and use **Re-run failed jobs**. The jobs that already
   succeeded keep their artifacts, and the failed one runs again.
2. If the run is too old to re-run, start **Release** by hand (Actions, Release, Run workflow)
   on the release's tag. A tag run requires CI to have passed on that commit.
3. A release page with no image yet is safe to leave for a day: the packages and checksums
   are already complete, and `latest` of the image only moves when its job succeeds.

A pre-release (a version with a suffix such as `2.6.0-rc.1`) is published as a GitHub
pre-release and never becomes `latest`, for the release page or for the image.

## Checklist: repository settings

Settings, General: allow squash merging only, default message "pull request title and
description", automatically delete head branches, allow auto-merge.
Settings, Rules, Rulesets: the `main` ruleset above, and a tag ruleset for `v*`.
Settings, Actions, General: workflow permissions read-only by default; require approval for
workflows from outside collaborators; only allow actions from GitHub and the ones listed.
Settings, Environments: `release` (the tag and package publication, with the maintainer as
required reviewer) and `pages` (deployment only from `main`).
Settings, Advanced Security: private vulnerability reporting, Dependabot alerts, secret scanning
and push protection.

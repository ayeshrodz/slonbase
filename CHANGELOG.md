# Changelog

All notable changes to slonbase are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/): since 1.0, breaking changes wait for a major version
(see [stability](docs/stability.md)). Before 1.0, a minor version (0.x) could contain breaking
changes, listed in its **Upgrading** section.

## [Unreleased]

### Security

- **Cron job settings are only shown to those who can change them.** `GET /api/crons` returned
  every job's settings to every role: an SQL statement, an email's text and addresses, a
  cleanup filter, a URL. Viewers and developers now see that a job exists, its schedule and how it
  went, and admins see the settings of every job except the SQL statement, which only owners
  see (as only owners can change it).
- **Only owners can run an SQL job by hand.** "Run now" on a job of type *Run SQL* needed only the
  developer role, which let a developer start a saved statement with the SQL console's reach.
  Developers and admins still run every other type.
- **The history no longer keeps what a called URL answered.** A *Call a URL* run recorded the
  first 2 KB of the response body, readable by every role through the jobs' history. It now
  records the status and the size. The content of runs recorded before this change is no longer
  shown either.
- **Headers stay on the origin they were meant for.** The headers of a *Call a URL* job were
  dropped on a redirect to another host, but kept on a redirect to another port or from https
  to http. They are now sent only to the same scheme, host and port.
- **A job's name can't carry line breaks or other control characters.** The name is used in the
  subject of failure notifications.
- **An email job can run at most every 5 minutes.** A schedule such as `* * * * *` would have
  mailed its recipients every minute.

### Fixed

- **Settings saved at the same time no longer overwrite each other.** Cron jobs, API keys, roles
  and the other settings were saved by writing back a whole copy read earlier, so two saves
  made close together (for example several jobs created at once, or a job saved while another
  setting changed) kept only the last one. Saves now read the stored settings under a lock and
  apply each change on top of them, on every instance.

### Security

- **Updated the bundled SQLite used by `slonbase import-pb`** (`rusqlite` 0.41.1 /
  `libsqlite3-sys` 0.39.1, SQLite 3.54.1) for CVE-2026-119375 (`RUSTSEC-2026-0459`). Building
  from source now requires **Rust 1.89 or newer** (was 1.88).

### Fixed

- **Lowering the number of automatic backups to keep takes effect immediately.** The limit
  (`backups.cronMaxKeep`) was only enforced after the next automatic backup, so lowering it left
  the older automatic backups in place until the next scheduled run. It is now also enforced
  when the setting is saved. Only automatic backups count towards the limit and are deleted by
  it; manual backups are never deleted, and the setting's help text now states this.

## [2.6.1] - 2026-11-09

### Security

- **A saved header value no longer follows a job to another address.** Editing a *Call a URL*
  job keeps its saved header values (shown hidden), but if the URL changes you now have to type
  the values again, so re-pointing a job can't send a saved `Authorization` header elsewhere.
  The same applies to a copy.

### Added

- **Start a cron job from an example.** The New job form has a row of examples that fill it in:
  delete records older than 30 days, ping a URL every 5 minutes (with two retries), a weekly
  email reminder, and (for owners, with the SQL console on) `ANALYZE` every Sunday. Nothing is
  saved until you press Make job.
- **Pause all cron jobs.** A "Pause all" switch on the Cron jobs page (setting
  `schedule.pauseJobs`, so every instance follows it) stops the jobs made in the dashboard from
  running on their schedule, for a maintenance window. "Run now" still works, a banner says
  they are paused, and jobs registered in hook files and the automatic backup are not affected.
  Resuming within the same minute still runs what is due.
- **Switch a cron job on or off, or duplicate it, from the list.** Each job made in the dashboard
  has an On/Off switch in the Cron jobs table (switching off always works, whatever else about
  the job has changed; switching on checks it again) and a Duplicate button that opens the
  editor with a copy ("Copy of …"), keeping the saved header values and webhook of the original
  (`copyOf` in `POST /api/crons`).
- **Cron jobs can tell you when they fail.** Set an email list (up to 10 addresses, through the
  mail settings) and/or a webhook (a public URL that gets a JSON `POST` with `event`, `job`,
  `error`, `attempts` and `at`) on a job. You get one message when it starts failing, after its
  retries, and one when it works again, not one every minute while it keeps failing. Runs you
  start with "Run now" don't send anything. A notice that can't be sent never fails the job. The
  webhook URL is treated like a header value: shown hidden after saving and kept when you edit
  the job. The API field is `notify` (`emails`, `webhook`).
- **Cron jobs made in the dashboard can retry.** A job can be set to try again up to 5 times when
  a run fails, waiting longer each time (the first wait is yours, 1 to 300 seconds; each next one
  is twice as long, at most 5 minutes). All tries are one run in the history, which says how many
  were made (`meta.attempts`, "after 3 attempts" in the failure). A failure that retrying can't
  fix, such as email not being set up, the SQL console being off or a refused address, isn't
  retried. The API fields are `retries` and `retryDelaySeconds`.

### Changed

- **Cron job history shows names and what a run did.** The History table listed a job's id
  (`user:a1b2c3…`) and left Details empty for the jobs made in the dashboard. It now shows the
  job's name and a summary of the run: records deleted, emails sent, the HTTP status, rows read
  or changed.

## [2.6.0] - 2026-11-09

You can make cron jobs in the dashboard now: Settings → Cron jobs → New job.

### Added

- **Cron jobs made in the dashboard.** Until now Settings → Cron jobs only listed the backup and
  the jobs a hook file registers with `cronAdd()` (as in PocketBase). Admins and owners can now
  make a job there, with a schedule (in the time zone of Settings → Backups) and a type. The
  first type is **Clean up records**: delete the records of a collection that match a filter
  (the API rules' syntax) and/or are older than some days, at most so many per run, with their
  files and cascades as when you delete them yourself. The editor shows the schedule in words,
  the next runs and how many records match right now. Jobs run once a minute they're due, on
  one instance, never twice at once, with a time limit, and their runs and results (how many
  were deleted) are in the history like the others. Developers can still run any job now. The
  API is `POST /api/crons`, `PATCH`/`DELETE /api/crons/{id}` and `POST /api/cron-preview`.
- **Cron job type: Call a URL.** A webhook or a health ping on a schedule: GET, POST, PUT, PATCH
  or DELETE with headers and a body, a time limit, and an optional status that counts as
  success (any 2xx by default). The status and the start of the answer are in the history.
  Only public addresses can be called: this server, private networks and cloud metadata
  addresses are refused when you save and again when it runs (every redirect is checked too, and
  a redirect to another host drops the headers). Header values (an `Authorization` token) are
  shown hidden after saving and kept when you edit the job.
- **Cron job types: Send an email and Run SQL.** *Send an email* sends a plain-text message to up
  to 10 addresses through the mail settings (SMTP has to be on; the editor says so otherwise).
  *Run SQL* runs a statement under exactly the SQL console's policy: the console must be on
  (`--sql-console`), it only changes data when writes are allowed (`--sql-console-writes`), and it
  runs as the console's role with its time limits. SQL jobs can only be made or changed by
  owners; the history records what a run did (rows, rows affected), not the rows it read.

## [2.5.3] - 2026-11-08

Schedules that can run on your own time zone, and a roles table that matches what is enforced.

### Fixed

- **Listing backups no longer fails (500) while a backup is being finished.** The list read the
  size of a file that had just been renamed from its temporary name, and the request failed;
  files that vanish while the folder is read are now skipped.
- **Schedules can be read on your own clocks.** Cron expressions (the automatic backup and the
  cron jobs of hooks) were always read in UTC, so `0 3 * * *` set on a machine in Auckland ran at
  16:00 there. The new `schedule.timezone` setting (Settings → Backups → Time zone, with a button
  for the browser's own zone) is `UTC` (the default, so **no existing schedule moves**) or a named zone such as
  `Pacific/Auckland`; it works the same on native installs and in Docker. Daylight saving time is handled the way Vixie cron does: a
  job at a fixed hour runs once when its time is skipped or repeated, and a job in every hour
  keeps to the clock. The dashboard shows each next run in that zone and in UTC.
- **The roles table in the dashboard was wrong about the SQL console.** Under Settings → Team it
  said developers could read with it; it has been for owners only since 2.4.0. The table is now
  computed by the server from the rules that enforce roles (`capabilities` in `GET /api/team`),
  so it can't disagree with them, and shows what each role can do in one clear matrix (one role
  at a time on a phone). The role pickers were also made readable on narrow screens.

### Changed

- **A new set of controls in the dashboard.** Buttons, fields, selects, switches and segmented
  controls are flat and a little rounder, in the same muted palette: a solid fill and a hairline
  border, no gradient or shadow. Checkboxes and radios use the tick of the project page: a soft
  accent fill with the mark drawn in, in the table's select-all, the export picker and the role
  chooser alike. Light and dark use the same shapes, and an installation's own accent colour
  still drives them.
- **Fixed on Settings → Login screen:** the Background control was narrower than its buttons, so
  "Image" stuck out of it; and the preview of the centered and minimal layouts showed a tiny card
  on a large empty background, and now draws the card closer.

### Packaging and project

- CI now runs the test suite on PostgreSQL 18 too (14 to 18), so "PostgreSQL 14 or newer" is
  tested on every major version, not only up to 17.

## [2.5.2] - 2026-11-08

**Safer releases and installs:** a software bill of materials and attestations with every
release, an installer that can be run again after a failure, a signing key only one job can use,
pinned workflow actions, and refreshed dependencies.

### Packaging and project

- **Every archive, package and container image now carries `LICENSE`, `NOTICE` and
  `THIRD-PARTY-LICENSES.md`** (the dashboard's libraries and fonts in full, and every Rust crate
  with its license and source). A check runs before anything is published.
- **The minimum Rust version is 1.88**, not 1.87: dependencies already needed it. CI now builds
  with the version `Cargo.toml` promises.
- **The installer can be run again after a failure.** It installs the slonbase package before
  creating anything in PostgreSQL, keeps the role's generated password in
  `/etc/slonbase/install.env` (root only) until setup succeeds, and a rerun reuses the role
  instead of stopping at "the role already exists". On Red Hat's family it adds password logins
  for its own role and database only, instead of switching every localhost rule to passwords.
  CI runs it in clean Ubuntu, Debian and Fedora containers: a failed package, a rerun after a
  failed setup, an upgrade that keeps the configuration, and a service that starts.
- **The apt and dnf repositories keep every supported line.** Instead of the five newest
  versions, they keep the newest three of each line that still gets fixes (2.5.x, and 2.4.x
  until February 7, 2027), so a 2.4 backport stays installable from the repositories however
  many 2.5 releases follow. `packaging/supported-lines.txt` lists the lines; CI checks it agrees
  with `SECURITY.md`.
- **Releases carry a software bill of materials and attestations.** Each release publishes
  `slonbase-<version>.spdx.json` (SPDX 2.3: the Rust crates and bundled libraries in the binary,
  with versions, licenses and checksums), and GitHub-signed attestations of where every archive,
  package and the container image were built and what they contain. Check them with
  `gh attestation verify` (see [Verify a download](docs/installation.md#verify-a-download)).
- **Releases are made with a pull request.** Actions → Prepare release opens "Release
  <version>": the version in `Cargo.toml`, the changelog section and its compare links (missing
  since 2.2.0, now rebuilt), the version pins in the docs and the home page, and, for a new
  minor, the supported lines and SECURITY.md's table. Merging it releases.
- **Every GitHub Action the workflows use is pinned to a commit**, with its version in a comment
  (`actions/checkout@3d3c42e… # v7.0.1`), so a moved tag can't change what runs in CI or in a
  release. Dependabot keeps the pins current.
- **The package signing key is used by one job only.** The Package repositories workflow signs
  the apt and dnf repositories in a job of its own, in the `package-signing` environment, and
  builds the page and the docs in another job that has no secrets. It also refuses to sign with
  any key but the published one, so a wrong secret fails the run instead of breaking
  `apt update` for everyone.
- **The support policy says what happens when minors come quickly:** each minor release keeps
  getting security fixes until three months after the next one came out, however many follow
  (2.4 until February 7, 2027, even if 2.6 comes sooner); fixes for an older line ship as its
  patch versions, never with features; there are no long-term support releases.
- Dependencies refreshed within their current versions (47 crates, among them `zip` 9.4,
  `flate2`, `encoding_rs`, `rustix` and the ICU crates); `cargo audit` reports nothing.
- The home page's binary install command no longer shows 2.2.0 when GitHub's API can't be reached.
- Home page layout: the deployment-layout tabs fit their content and sit centered over the
  diagram; on phones, the developer-experience section is centered like the others, and the
  "Choose … when" comparison cards stack in one column instead of three squeezed ones.
- The security policy supports 2.5.x and 2.4.x (until February 7, 2027); `SUPPORT.md`,
  `MAINTAINING.md`, `CODEOWNERS` and Dependabot configuration are new.

### Security

- **More tests, no problem found:** every OAuth2 provider preset is well formed (https endpoints, no
  credentials in URLs), and the upgrade test now carries a project across the upgrade and checks
  its data, its superuser and its separation from the main installation.

## [2.5.1] - 2026-11-07

A query time limit for slow views and heavy filters, and tests on PostgreSQL 14 to 17.

### Security

- **`database.statement_timeout`:** seconds after which PostgreSQL ends a query of slonbase's
  connections (0, the default: never; backups and restores are exempt). A view that is slow to read
  or a heavy filter no longer has to hold a pooled connection for as long as it runs.
- **More tests, no problem found:** imports that change schemas (type changes, dropped columns,
  invalid collections, dropped referenced collections) and failed replacements of uploaded files.
  CI now runs the tests on PostgreSQL 14, 15, 16 and 17.

## [2.5.0] - 2026-11-07

Following up the two audits: an open realtime stream now ends with its session, every API route
needs a deliberate role decision, and more tests that hold the line. See
[the security reviews](docs/security-review.md#following-up-the-audits-250).

### Security

- **A first OAuth2 sign-in that loses a race no longer leaves a record behind:** if linking the
  provider fails after the record was created, the record is removed.
- **Tests that hold the line, no problem found:** credentials of one installation (main, projects,
  tokens, API keys) mean nothing in another; hostile upload file names stay inside storage;
  backup extraction never writes outside its directory (zip-slip, absolute names, symlinks).
- **An open realtime stream no longer outlives its session.** A client subscribes with its token once and keeps
  receiving events; ending that session (revoking its tokens, changing its password) or deleting
  the account didn't stop the stream until the client disconnected. Each delivery now checks that the
  account still exists with the same token key, and closes the streams of those that don't. Found
  by a new test (`open_streams_follow_rights_changes`) that also checks a tightened rule stops
  the events. A test now also guards the role rules: every API route needs a deliberate
  decision in `roles.rs` (`POLICY`), so a new route can't be left out of the role checks unnoticed.

## [2.4.1] - 2026-11-07

Follow-up to 2.4.0's security hardening: a trusted proxy list, a bounded rate limiter, and tests that
were checked to fail when the protection they cover is removed.

### Security

- **The in-memory rate limiter stays bounded under a flood of distinct clients.** Before, once
  200,000 counters were all still running, every request rescanned the whole map (CPU) and new
  clients kept being added (memory). Now it sweeps at most once a second and, while full, doesn't
  track new clients (those already counted stay limited). Found by a test that hung.
- **`trustedProxy.peers`:** the addresses (IPs or CIDR ranges) of the proxies allowed to set the
  forwarded-address headers. When it is set, a request from any other address has its headers
  ignored, so a client can't forge its address to dodge rate limits and lockouts; trusted hops are
  skipped from the right. Health warns when headers are trusted but no peers are listed.
- **Tests that can fail:** the SQL console test expected a 403, which a console that is switched off
  also returns, so it would have passed with the role check removed; it now runs with the console on
  and fails without the check. Backup retries and the file swap of a restore (the old files must
  stay if it fails) are tested too.

## [2.4.0] - 2026-11-07

**Security hardening: safer defaults, checks that say when a setup is weak, and tests that hold
every path to the same rules.** Prompted by an outside review of the code.

### Upgrading

- **The sign-in lockout is now on, everywhere:** 10 failed password sign-ins per identity and
  address in 15 minutes. Installations that had it at 0 (the old default) are switched on once,
  at the first start of 2.4; turn it off again in Settings, Security, if you really need to
  (Health will warn).

### Security

- **Rate limits keep their total when the database fails.** With several instances, requests are
  counted in PostgreSQL; if that failed, each instance counted on its own, so three instances
  allowed three times the limit, and only a debug log said so. Now each instance counts its
  share of each limit (the limit divided by the live instances), the failure is logged as a
  warning, and Health reports it.
- **Health has security checks:** slonbase connecting as a PostgreSQL superuser, the lockout
  off, rate limits off or recently degraded, and an application URL without https.
- **The SQL console can run as a role of its own:** `--sql-console-role`
  (`SLONBASE_SQL_CONSOLE_ROLE`, `server.sql_console_role`) runs its statements with
  `SET LOCAL ROLE`, so it can have fewer privileges than slonbase, such as read-only access
  (see [configuration](docs/configuration.md#server-configuration)).
- **One test for every way to reach a record:** `tests/authz_matrix.rs` checks that list, view,
  counts, filters (through relations and `@collection`), forward and back `expand`, protected
  files, realtime, update, delete and batch all agree with the same rules, for a guest and two
  users.
- **Generated filter tests:** thousands of filters full of SQL and filter metacharacters are
  compiled, checking that a client's value never reaches the SQL text, only parameters, and
  20,000 random inputs that the parser and compiler never panic.
- **A restricted dashboard role could reach what it forbids by another route** ([GHSA-8g53-r6rc-fm83](https://github.com/ayeshrodz/slonbase/security/advisories/GHSA-8g53-r6rc-fm83); reported by two
  outside reviews, reproduced with tests before fixing; see the
  [security reviews](docs/security-review.md)). A developer could create a superuser (which
  starts as an owner) through the superusers collection's *id* or through a *batch*; a read-only
  *API key* could change its owner's password; an *import* could carry the superusers
  collection; the *SQL console* was open to developers and admins, and reads every table.
  Roles now apply to a collection however it is addressed, to every request in a batch and an
  import, API keys can't change the account they belong to, and the SQL console is for owners.
- Backups extract within the free disk space and a limit of entries, and a failed automatic
  backup is retried; a restore stages the files before the database changes; CI runs on pull
  requests and fails, rather than skips, without its database.
- A [threat model](docs/security.md): what slonbase protects and trusts, how each boundary is
  enforced and tested, and how to check a deployment. `SECURITY.md` invites independent review.

## [2.3.1] - 2026-11-07

**slonbase is now licensed under the Apache License 2.0.**

### Changed

- **License: Apache 2.0** (was MIT) for everything released from this version on, with an
  explicit patent grant. Versions up to and including 2.3.0 were released under MIT and remain
  available under it. The dependencies are all under permissive licenses. See
  [license and contributing terms](docs/licensing.md).
- **Contributions need a signed [CLA](CLA.md):** a bot asks on a first pull request. Contributors
  keep their copyright; the agreement licenses their work under Apache 2.0 and lets the
  maintainer license it under other terms in future. Everything released under Apache 2.0 stays
  under it; the maintainer may one day offer separately licensed commercial features or hosted
  services, which the CLA makes possible.

### Added

- [`NOTICE`](NOTICE) (third-party notices), a [trademark policy](TRADEMARKS.md) for the name and
  logo, a [license guide](docs/licensing.md), and the license named in the package metadata, the
  README, the site, `llms.txt` and the structured data.

## [2.3.0] - 2026-11-07

**Several instances, tested properly.** Running slonbase as a fleet, behind a balancer, behind
PgBouncer and with projects on other servers, was run for real (`scripts/fleet-test`, also in CI),
which found problems that are fixed here, and a [scaling guide](docs/scaling.md) now says what
works, what was measured and where the limits are.

### Added

- **`database.listen_url`** (`--database-listen-url`, `SLONBASE_DATABASE_LISTEN_URL`): a direct
  connection for `LISTEN/NOTIFY`, so slonbase works behind PgBouncer in transaction mode. Without
  it, behind such a pooler, instances started and answered requests but stopped sharing schema
  changes, settings, realtime events and rate limits, with no error.
- **Health checks for a fleet:** **sync** (each instance sends itself a notification every minute
  and shows an error, naming a transaction-mode pooler and the fix, if it doesn't arrive) and
  **connections** (PostgreSQL's connections in use against its limit, a warning from 80% and an
  error from 95%, since several instances and projects multiply them).
- **Sign out everywhere:** `POST /api/collections/{collection}/records/{id}/revoke-tokens` gives a
  record a new token key, so every token it holds stops working on every instance; a user may
  call it for themselves, a superuser for anyone. The dashboard's record menu has the action.
  (Tokens are stateless, so there is no per-device logout, as in PocketBase.)
- **[Scaling and deployment topologies](docs/scaling.md):** five layouts with diagrams (one
  instance; projects on several servers; a load balancer; PgBouncer; managed failover), the
  connection budget N × (18 + 5 × projects) with measured numbers, what is shared and what is per
  instance, sessions and sign-out across instances, upgrades, nginx, HAProxy and PgBouncer
  recipes, and the limits.
- **`scripts/fleet-test`:** several instances behind a balancer with no affinity, checking tokens,
  schema and settings propagation, shared rate limits, realtime forwarding and projects, run in
  CI directly and through PgBouncer.

### Changed

- **Idle project pools release their connections** after two minutes, so many projects don't each
  hold a few connections for nothing.
- **Pooler-safe session state:** the backup lock is a transaction lock on a dedicated connection,
  and the SQL console's and JavaScript hooks' `SET`s are `SET LOCAL` inside a transaction, so
  nothing leaks onto the next user of a pooled connection (a leak that was possible even without a
  pooler).
- A realtime subscribe request that was forwarded to an instance that has died answers after two
  seconds instead of five.
- The website's scale-out section and the documentation say what was tested, including the
  limits (connections, local files, no rolling upgrades).

### Website, documentation and discovery

#### Added

- **Easier to find, for search engines and for AI assistants:** a sitemap, a `robots.txt` that
  welcomes search and AI crawlers, canonical addresses and Open Graph cards on every page,
  structured data (software, FAQ, breadcrumbs, articles), a 404 page, `llms.txt` and
  `llms-full.txt` with every guide also served as Markdown (`/docs/<page>.md`), and an IndexNow
  ping on each deploy.
- New guides: [About](docs/about.md), [Features](docs/features.md), a five-minute
  [quickstart](docs/quickstart.md), [slonbase and PocketBase](docs/slonbase-and-pocketbase.md),
  [slonbase vs PostgREST](docs/slonbase-vs-postgrest.md) and the [benchmarks](docs/benchmarks.md).

#### Changed

- **slonbase is described as what it is now:** a backend platform on PostgreSQL (many projects,
  roles and API keys, an SQL console, designers, scale-out), compatible with the PocketBase SDKs,
  not "PocketBase's API on PostgreSQL". The README, the site, the package descriptions and the
  documentation lead with that.
- The documentation site uses the dashboard's colours, and passes the accessibility checks.

- **A new look for <https://slonbase.com>,** built from the dashboard's own colours, type and
  components, with a screencast of the real dashboard, every dashboard screen (the command
  center for projects, the SQL console, settings and the email designer included) and the
  benchmark numbers of 2.2.
- **Benchmarks of 2.2.0** against PocketBase 0.40.6 and PostgREST 16.5, with memory, binary size
  and start-up time (`bench/`; the runs are kept in `bench/published/2.2.0`). The binary is 22 MB
  on disk (10 MB to download), not the 8 MB the page said.
- The dashboard screenshots are taken, and the tour recorded, by `scripts/demo/run.sh`, from a
  seeded demo installation.

## [2.2.0] - 2026-11-07

**slonbase.com, and an installation that starts from a bare server.**

### Added

- **`install.sh`:** `curl -fsSL https://slonbase.com/install.sh | sudo sh` takes a Debian,
  Ubuntu, Fedora, RHEL, Rocky or Alma server to a working instance: it installs PostgreSQL when
  there is none (a 14 or newer; on RHEL 8 and 9 it enables the 16 stream and moves localhost
  logins from `ident` to passwords), creates a role and a database for slonbase with a generated
  password (never touching an existing role or database), adds the signed repository, installs
  the package and creates the first superuser. It prints each step and the sign-in at the end,
  takes its choices from environment variables (`SLONBASE_EMAIL`, `SLONBASE_DATABASE_URL`, ...)
  and is safe to run again. It is checked with ShellCheck in CI.
- **A guide from zero:** the installation guide starts with that command, then does each step by
  hand (PostgreSQL, the role and database, the package, the setup), and ends with a check that
  it works and a troubleshooting table. Docker and Docker Compose are their own sections.

### Changed

- **The project lives at <https://slonbase.com>** (the apt and dnf repositories, the docs and the
  page). Old `github.io` addresses redirect, so existing repository entries keep working.
- The documentation, README, site and every dashboard screenshot are brought up to date with the
  current release (the screenshots were from 2.0, before the layered surfaces); the package's
  post-install message says that a database is needed and points at the installer.

## [2.1.3] - 2026-11-07

**A leaner release pipeline.** The same binary as 2.1.2; only how it is built and published
changed.

### Changed

- **The apt and dnf repositories keep the five newest versions.** They carried every release,
  and their GitHub Pages deployment grew past the 1 GB limit, so publishing 2.1.2 to them
  timed out. Older versions stay on their [release pages](https://github.com/ayeshrodz/slonbase/releases)
  as `.deb`, `.rpm` and `.tar.gz` files and as Docker images.
- Releases build their packages once, and update the repositories once. CI no longer builds the
  packages or the image from source on every push (the release does, before it publishes), and
  its jobs that run slonbase share a single build.

## [2.1.2] - 2026-11-07

**A dashboard whose parts you can tell apart.**

### Changed

- **Layered surfaces.** The light theme's page is a soft grey with white cards, inputs and
  tables on it, and darker wells (table heads, code) in them; the sidebar is a step darker than
  the page; cards have a faint shadow. In the dark theme the cards and inputs are lighter than
  the page, and table heads lighter again. Before, the page and the cards were the same white
  (or the same black) and only hairlines told them apart.
- **Controls follow the accent.** Checkboxes, radio buttons and sliders take the installation's
  accent (they were always the browser's colour), as does the endpoint's method label.
  Status colours stay green, amber and red whatever the accent.
- **Settings → Mail: the preview is below the editor,** as wide as it, instead of beside it in
  half the width.
- **The overview's collections scroll** inside their card (with a filter once there are more
  than ten) instead of making the page as long as the list.

## [2.1.1] - 2026-11-07

**A dashboard that stays usable during bulk imports, and honest about logging.**

### Fixed

- **A bulk import made the dashboard laggy.** A batch holds a database connection for its whole
  transaction, and a project's pool is small (4 by default), so a few import workers took every
  connection and the dashboard's own requests waited behind them. Batches and uploads now take
  turns, using all but one connection of the pool; the rest wait, instead of failing on the
  pool's timeout, and one connection is always free for everything else.
- **The overview showed a confident low request count when request logging was off.** Requests
  are counted from the request logs, so with logging off (`logs.maxDays` of 0, as an import
  script may set) it counted nothing. The overview now says "Not logged" and links to the
  setting, Health warns, and the command center shows "No logs" on the installation.
- **Settings → Application had two save buttons** for the accent: its own, and the settings
  bar's. The accent is saved with the bar, like everything else; Reset preview stays.
- **App users → New auth collection** opened the dialog with the Base type. It starts with
  Auth now.

## [2.1.0] - 2026-11-06

**The rest of 2.0's plan: files, sign-in protection and a rate-limit tester.**

### Added

- **Lockout after failed sign-ins** (`security.loginLockout`): an identity that fails
  `attempts` password sign-ins from one address within `minutes` gets `429` until the window
  ends, even with the right password; a success clears the count. Shared by every instance.
  Off by default.
- **The interval between auth emails is a setting** (`security.authEmailInterval`, 60 seconds
  by default, 5 to 86,400): verification, password reset, email change and one-time
  passwords, per account.
- **Files:** `files.defaultMaxSize` sets the largest file for file fields without a size of
  their own (it was always 5 MB), and `files.thumbs` turns thumbnails off (the original file
  is served instead). Settings → File storage shows both, and each auth collection's session
  and protected-file token lifetimes; Team shows the superusers'.
- **A rate-limit tester:** Settings → Security & limits says which rule a request would meet
  (`POST /api/settings/rate-limits/explain`), with the same matcher the guard uses.

## [2.0.0] - 2026-11-06

**Settings in which every setting does something.** The Settings page is rebuilt: an
appearance with named accents, the application's name and images used everywhere, a sign-in
page of your own, an email designer, roles for superusers and API keys, a history of what
backups and cron jobs did, CORS, maintenance, and more of logs, storage and import. Each
installation (the main one and every project) has its own.

### Upgrading

Nothing needs doing: existing settings and superusers carry over. What behaves differently:

- **The API sends CORS headers**, allowing pages on any site, as PocketBase does; slonbase sent
  none before, so browsers refused cross-origin calls. To allow only some sites, list them in
  Settings → Security & limits (`security.allowedOrigins`).
- **Emails are put in a layout** (the logo, a card, the application's name and address), unless
  a template is a whole HTML document of its own. Turn it off, or change it, in Settings → Mail
  (`mailLayout.enabled`).
- **Every superuser is an owner** until you give them another role, so nobody loses access.
- **Two migrations** add `_slonbase.assets` (images, included in backups) and
  `_slonbase.job_runs` (the jobs' history). A backup made with 2.0 can't be restored by 1.x.
- `GET /api/crons` items have more fields (`description`, `next`, `kind`, `lastRun`);
  `GET /api/backups` items have `encrypted` and `auto`.
- The default accent is `sage` (the colour the dashboard already had); saved colours stay.
- For the Rust library API: `RequestAuth` has a `role` field, and `logs::cleanup` takes the
  maximum number of entries.

### Added

- **Settings, reorganized.** Sections are grouped (Workspace, Delivery, Operations,
  Protection, Data), with a dot on those with unsaved changes. Links to a section (from Health's
  checks, the command palette) keep working.
- **Appearance.** Settings → Application has cards for the color mode (system, light, dark;
  kept per browser) and thirteen named accents (Sage, Slate, Mauve, Clay, Sand, Teal, Ocean,
  Indigo, Rose, Forest, Amber, Plum, Graphite), each with a light and a dark tone that stay
  readable (WCAG AA) in either theme, or a color of your own, with a preview to try them before
  saving. The accent is the installation's: it applies before the first paint, on the sign-in
  page too, and each project has its own.
- **The application's name means something.** It names the browser tab (`Logs · Acme`), the
  sidebar and the sign-in page, besides emails.
- **Branding: images and logos.** Each installation keeps a library of images (PNG, JPEG,
  WebP, GIF, ICO or SVG, up to 2 MB, up to 100) at `/api/branding/assets`, and chooses its logo,
  its logo on dark backgrounds and its browser icon from it. Anyone may view an image (the
  sign-in page and emails show them); only superusers upload, rename or delete them, and an
  image the settings use can't be deleted. They live in the database, so they go with backups.
- **A sign-in page of your own.** Settings → Login screen sets its layout (beside the code
  panel, in a card, or alone), background (plain, a grid or an image), heading, line under it,
  logo, a notice (plain text), up to three links (`https:` or `mailto:`) and whether
  "powered by slonbase" shows, with a live preview of the real page.
- **A history of what ran.** Automatic and manual backups and cron jobs (hooks' `cronAdd` and
  the backups' schedule) record each run: when, what started it, how long it took, how it
  ended (with the error when it failed) and, for backups, the file, its size and whether it's
  encrypted. The last 500 runs of each job are kept (`GET /api/jobs/runs`).
- **Backups say whether they're encrypted** (`encrypted`, from the file's first bytes, in
  `GET /api/backups`) and whether they're automatic; the page filters them and shows when the
  next automatic one runs.
- **Cron jobs, at a glance:** each job's schedule in words ("every day at 03:00"), its next run
  and how its last run went (`GET /api/crons` has `description`, `next` and `lastRun`), with
  its history.
- **File storage shows where files are:** the directory on the server (or the bucket), the free
  space there, what the uploads take (measured in the background, by collection) and
  thumbnails, which can be cleared (`GET /api/settings/storage`).
- **CORS.** Pages on other sites may call the API, as with PocketBase; Settings → Security &
  limits can restrict that to some origins (`security.allowedOrigins`). Until now, slonbase
  sent no CORS headers, so browsers refused such calls.
- **More of Security & limits:** maintenance (the API answers 503 with your message to all but
  superusers and health checks), HTTPS only (`Strict-Transport-Security`, when the application
  URL is https), and the largest request (128 MB by default, now a setting).
- **More of Logs:** paths not to log (unless they fail on the server), slow requests logged as
  warnings whatever the minimum level, and a maximum number of entries.
- **Import & export:** choose which collections to export (system ones too, indented or not, to
  copy or download); an import shows, collection by collection, the fields added, changed and
  removed, the rules and indexes it changes, and what Replace would delete; and the settings
  can be exported and applied elsewhere (without passwords, secrets or images).

- **An email designer.** Settings → Mail edits the layout around every email and each auth
  collection's messages (verification, password reset, email change, one-time password, new
  sign-in alert) as HTML, with buttons to insert placeholders, a live preview of what the
  server would send (desktop or phone width, in a sandboxed frame that runs nothing), and a
  test send from the collection's own template (the dashboard used to always send the
  superusers').
- **A layout around every email** (`mailLayout`, on by default): the logo, a card in the
  accent's color, the application's name and address. A template that's a whole HTML
  document of its own isn't wrapped.
- **New placeholders in emails:** `{APP_LOGO}` (the logo, or the application's name),
  `{APP_LOGO_URL}`, `{ACCENT}` and `{RECORD:field}` (the recipient's fields, escaped; hidden
  fields, the password and the token key stay empty). `POST /api/settings/mail/preview`
  renders an email with sample values.

- **Roles for superusers.** Each superuser of an installation is an owner (everything, as
  before), an admin (everything but the team, restores and deleting projects), a developer
  (collections, records, files, the SQL console when it only reads, cron jobs) or a viewer
  (looks at everything but backups' files, changes nothing). Roles are checked for every
  request, extensions' routes included; everyone may still sign in, refresh their session and
  change their own password. There's always an owner. Settings → Team lists the superusers,
  adds them and changes their roles (`GET /api/team`, `GET /api/team/me`,
  `PATCH /api/team/members/{id}`).
- **API keys** (`sb_…`, sent as a bearer token): for scripts, CI and servers, each acts for the
  superuser who made it, with a role up to theirs, and optionally an expiry date and IPs it
  works from. Only a hash is kept; the key is shown once. They can't make file tokens
  (`POST /api/team/keys`, `DELETE /api/team/keys/{id}`).
- **Signing in to the dashboard with OAuth2:** the sign-in page shows a button per provider
  set up on `_superusers`; only existing superusers whose verified email matches get in.
- **App users:** Settings → App users shows the auth collections your application signs its
  users in with: how many users, which ways to sign in are on, which OAuth2 providers are
  configured, and the redirect URL to register with providers.

### Security

- **Uploaded images can't run anything.** An image's type comes from its bytes, not from what
  the upload says it is. SVG images with scripts, event handlers, animations, embedded
  documents, DTDs or links outside the file are refused, and every image is served with
  `X-Content-Type-Options: nosniff` and a sandboxing `Content-Security-Policy`.
- **Backups' files are for owners and admins**, however the request is made (with a file token
  too).

## [1.11.1] - 2026-11-06

**Each installation keeps its own SQL history, and every page has the same width.**

### Fixed

- **The SQL console's history was shared between the main installation and its projects.**
  Projects under the main installation's address share its browser storage, so queries run in
  one showed up in the history of every other. The history, the row limit and a collection's
  hidden columns are now kept per installation; the main installation keeps the history it has.
- **Pages had different widths:** the Projects page and the SQL console were wider (1,600px)
  than the other pages (1,320px), and a collection's pages (1,480px) in between. Every page is
  now up to 1,440px wide with the same margins; forms (API preview, rules, indexes, auth
  options) keep a readable 900px column inside it, and the records grid still uses the whole
  width.

## [1.11.0] - 2026-11-05

**A command center for every project, and an SQL console you can trust.** The main
installation's Projects page shows and controls every installation it hosts, live; projects can
connect a database on any PostgreSQL server; and the SQL console only reads unless allowed to
write, with limits, a schema browser and exports.

### Security

- **The SQL console only reads unless it's told it may write.** With `--sql-console` alone it
  now runs one read statement at a time (`SELECT`, `WITH`, `SHOW`, `EXPLAIN`, `VALUES`,
  `TABLE`) in a read-only transaction; `--sql-console-writes` (`SLONBASE_SQL_CONSOLE_WRITES=true`,
  `server.sql_console_writes`) lets it change data, as before. If you used the console to
  write, add that setting. Before, a read could end its read-only transaction and write after
  it (`SELECT 1; COMMIT; DELETE ...`), since several statements ran as one.
- **Console queries have limits:** rows are read through a cursor (1,000 by default, at most
  5,000 and 16 MB), statements stop after 30 seconds (it was 3 minutes) and after 5 seconds
  waiting for a lock, queries are at most 20,000 characters, two run at a time per instance,
  and each runs on a connection of its own that is closed afterwards. A query asking for a
  large table used to be read whole into memory before being cut to 1,000 rows.

### Added

- **A command center for projects.** The main installation's Projects page shows every
  installation it hosts, the main one included, updated every 5 seconds: fleet totals (running,
  requests, server errors, the slowest p95, data, records, realtime clients), requests per
  minute over the last hour with a line per installation, how traffic, storage and records
  divide between them, and a card (or a sortable table row) per installation with its
  sparkline, numbers, connections, database size and round trip, and configuration at a glance.
  A drawer per installation has its charts, its settings (application, sender, SMTP, backup
  schedule, log retention, rate limits), its connection, and restart, host names and delete.
  Projects have no such page: only the main installation sees and controls them all.
  `GET /api/projects/overview`, `GET/PATCH /api/projects/{name}/settings` and
  `POST /api/projects/{name}/restart` are the API behind it.
- **Projects on other PostgreSQL servers.** A project can connect an existing database by URL
  (`"databaseUrl"` in `POST /api/projects`, `slonbase project create --connect`, or **Connect a
  database** in the dashboard) instead of creating one on the main server. A database that
  already holds a slonbase installation keeps its data and superusers. The URL is stored
  encrypted with the settings key and never sent back; deleting the project leaves the
  database as it is. ([details](docs/projects.md#databases-on-other-servers))
- **A proper SQL console.** A table browser with each collection's columns (click to insert,
  or query a table in one click), an editor with line numbers, highlighting, Tab and
  Ctrl+/ (comment), the rows to fetch, and results below it: row numbers, column types, NULLs,
  a cell viewer, and errors that point at their line and column. Results export as Excel
  (`.xlsx`), CSV, TSV or JSON, or copy as CSV or JSON. `GET /api/sql` says whether the console
  may write, and its limits.

### Fixed

- **`slonbase rekey` with projects that have their own role.** It re-sealed the main database
  first and then read the projects' role passwords with the old key, so changing the key failed
  for those projects; it now reads them first.
- **Collections by their names in the SQL console.** `SELECT * FROM clients` works: the console
  searches the `slonbase` schema, where collections' tables are, before `public`.
- **The Health page's errors check no longer counts refused requests.** A 401 from an expired
  session or a 403 from the disabled SQL console is logged at the error level (as PocketBase
  logs it), but isn't something to fix; the check now counts server errors (5xx) and errors
  outside requests (hooks, crons, mail), and its link opens just those in the logs.

## [1.10.0] - 2026-11-05

**A dashboard that looks like the developer tool it is.** New type (IBM Plex Sans and
JetBrains Mono, with condensed capitals for titles), a status bar, the API URL on the
overview, neutral GitHub-like colours, flat buttons and far less motion.

### Changed

- **The dashboard, redesigned as a developer tool.**
  - Type in the manner of IBM's and JetBrains' tools: IBM Plex Sans for the interface, and in
    its condensed width, semibold and in capitals, for titles; JetBrains Mono for small-capital
    labels over sections, columns and figures, and for ids, collection names, numbers and code.
    They replace Ubuntu Sans and ship with the dashboard (168 KB). The wordmark stays
    lowercase.
  - A status bar along the bottom, as in an editor: whether the server answers, the PostgreSQL
    version and round trip, the API base and the version, refreshed every 30 seconds. The
    overview shows the API URL to point clients at, with a copy button, and the sign-in page
    shows the PocketBase SDK at work.
  - Neutral, GitHub-like colours: white and `#f6f8fa` in light, `#0d1117` ink, hairline
    borders and GitHub's status colours in dark; tighter corners, no decorative glow. The
    current item in the sidebar and tabs has a hairline and an accent marker.
  - Buttons are flat and exact: a hairline, a colour change on hover and a darker press, a
    2px focus outline; nothing lifts, scales or shines.
  - Far less motion: no page or title entrances, staggered lists, springy icons, pointer
    spotlight on cards, counting numbers or growing bars. Menus, dialogs, drawers and toasts
    still fade in briefly, and a changed row still flashes.
  - Still no serious or critical axe (WCAG 2 AA) violation on any checked screen, in either
    theme.
  - The documentation site and the project page use the same fonts, and the README's
    screenshots show the new dashboard.

## [1.9.0] - 2026-11-04

**Closer to PocketBase, and checked against it.** View fields are copies of their source
fields, as PocketBase makes them; VK completes PocketBase's sign-in presets; six answers that
differed from PocketBase's now match; health checks and single-record reads cost less; and the
dashboard passes an accessibility check, kept in CI with screenshot comparisons.

### Added

- **View fields copied from their sources.** A view column taken from a collection is a copy of
  that field: a select keeps its values, a relation its collection, a file field its options,
  and a collection's `id` under another name is a relation to it. Where each column comes from
  is PostgreSQL's own column origin, so joins, subqueries and CTEs work without parsing the
  query. Computed columns still get a field of their type (`count(*)` is now a whole number).
  Options set on the view's fields are kept, a hidden source stays hidden, and an auth
  collection's password hash is hidden text.
- **Files of view records.** A view record's file is served from the record it came from (the
  one with the same id, or else the one whose field holds that file name), after the view's
  rule and its own field are checked.
- **Sign in with VK**, the last of PocketBase's OAuth2 presets: every one is now there.
- **Dashboard checks in CI.** On a fresh install with a fixed data set, axe-core (WCAG 2 A and
  AA) runs on the sign-in page, overview, records, record editor, schema, rules and settings
  in both themes, and their screenshots are compared with committed baselines
  ([ui/test](ui/test/README.md)).

### Changed

- **Cheaper health checks.** The database probe is shared for a second: requests in that
  second (or while a probe runs) get its outcome instead of each running a query. An outage
  still shows within a second. 1.43 times the requests per second of 1.8 on the
  [benchmark](bench/README.md) (1.47 in alternating runs), now 1.14 times PocketBase's.
- **Cheaper single-record reads.** A record with nothing to change it on the way out (no view or
  enrich hooks, no `expand`) is written straight from its row, as lists are, and the column
  list is built in one buffer: about 136,600 instructions instead of 172,000, 1.13 times the
  requests per second with 32 clients in alternating runs, and 1.08 times with one.
- **Readable text everywhere in the dashboard.** Muted and small print, chips, statuses and links
  now reach a 4.5:1 contrast in light and dark mode (some were at 2.5 to 4.4:1), while fills,
  dots and charts keep their colours; the page-size select has a label.

### Fixed

Found by the [differential runner](compat/README.md#differential-runs), which now also covers sign-up, sign-in,
confirm tokens, rules for signed-in users, batches and error codes; now as PocketBase answers:

- A sign-in without identity or password names the missing fields (`validation_required`)
  instead of a bare 400.
- `auth-methods` also sends `authProviders`, `usernamePassword` and `emailPassword`, which
  older SDKs read.
- An invalid verification, password reset or email change token gets
  `validation_invalid_token` on `token`.
- Changing an email directly without manage rights gets `validation_values_mismatch`.
- A missing related record is reported together with the other field errors, not only once
  everything else is valid.
- Previewing a view whose query isn't a SELECT is a plain 400, as for queries the database
  refuses.
- A debug-only assertion in local storage panicked when the storage path itself contained `..`
  (release builds weren't affected).

## [1.8.0] - 2026-11-03

**Safer 1.7, and faster creates.** A security review of what 1.7 added, with three fixes;
records are created in one round trip (1.31 times as fast as in 1.7 on the benchmark); and the
realtime outbox reader is cheaper when idle and unaffected by long transactions.

### Security

From a review of what 1.7 added ([details](docs/security-review.md#what-17-added-180)):

- **View previews close their connection afterwards.** A previewed query that took a
  session-level advisory lock (`pg_advisory_lock()`) left it held on a pooled connection after
  the read-only transaction rolled back. Only superusers can preview, with the SQL console
  enabled.
- **More answers left uncompressed.** Besides `"token"`, any JSON answer with a key such as
  `accessToken`, `refresh_token`, `idToken`, `secret`, `clientSecret`, `password` or `apiKey` is
  never gzipped, so a custom route that signs users in is safe from BREACH whatever it calls its
  token.
- **A transaction left open no longer slows realtime.** The outbox reader scanned every event
  committed since the oldest open transaction began, on every read; it now looks up only the
  transactions it hasn't seen finish (two index lookups), whatever is left open.

### Changed

- **Creates in one round trip.** A create without an execute hook around it and without a
  create rule checked against the new record runs without BEGIN and COMMIT, which add two
  round trips and nothing else: 3,831 creates per second against 2,929 for 1.7.0 on the
  [benchmark](bench/README.md) (1.18 times in alternating runs on the same database), and 1.37
  times PocketBase's (1.72 times with asynchronous commits). Updates and deletes keep their
  transaction.
- **Realtime costs less when idle.** Each instance reads the outbox every 500 ms after
  activity, backing off to every 5 s while nothing happens (it was every 500 ms always); writes
  and notices still wake it at once. A commit that may have run SQL writes (a hook's
  `$app.db()` in a transaction) wakes it and the other instances too.

## [1.7.0] - 2026-11-03

**A new dashboard, and writes that commit in parallel.** The dashboard is redesigned from the
sidebar up; creating records is 1.64 times as fast as in 1.6; JSON answers are compressed;
view queries are previewed before they're saved; and every push is compared with PocketBase.

### Added

- **Previews of view collections.** `POST /api/collections/meta/dry-run-view` returns the
  fields and first rows a view's query would give, without saving it; the dashboard's view
  editor shows them as you type. It needs the SQL console (like saving a view), runs the query
  read-only with a 15-second limit, and refuses queries that can't make a view (PostgreSQL's
  error, or empty or repeated ids) with the reason.
- **Compressed JSON answers**: gzip, for answers over 1 KB, when the client accepts it. A page
  of 30 records goes from 14.5 KB to 2.2 KB for about 25 µs of CPU. Answers carrying a token or
  a secret are never compressed, so they can't be guessed from the compressed length (BREACH):
  sign-ins, refreshes, impersonation, file tokens, setup, Apple client secrets, and any JSON
  answer with a `"token"` key (a custom route's sign-in, say). `--no-compression` (or
  `server.compression = false`) turns it off.
- **A differential runner** (`compat/diff/run.sh`), run in CI on every push: PocketBase 0.40.5
  and slonbase side by side, the same seed and requests, the answers compared as JSON, plus
  1,000 fuzzed HTML documents through both servers' `:excerpt`.

### Changed

- **A redesigned dashboard.** One sidebar holds the sections, the collections (with a filter
  once there are many), the theme switch (light, dark or the system's) and the account; it
  folds to icons and becomes a drawer on small screens. The top bar and the separate
  collections panel are gone. A warm, muted palette (paper and stone neutrals, a sage accent,
  a charcoal dark theme) with soft shadows in place of borders; pill tabs; a calmer records
  grid; records open in a drawer beside the list instead of a dialog over it. The overview
  shows the day's traffic as a chart next to the greeting, the collections ranked by their
  records, and a health checklist; the sign-in page has a panel about slonbase beside the
  form. The default accent is now sage; an accent picked in the settings is kept.
- **Writes commit in parallel.** Realtime events used to be announced with a `pg_notify` in
  every write transaction, and PostgreSQL commits notifying transactions one at a time. Each
  instance now reads new events from the outbox by transaction snapshot (migration 15 adds
  each event's transaction id), woken at once by its own writes; other instances get one
  coalesced notification, sent outside the writes, and every instance also checks every
  500 ms. Creating records is 1.64 times as fast as in 1.6 ([benchmarks](bench/README.md)).
  Writes from other programs still reach realtime subscribers, as before.

### Upgrading

- **Several instances on one database: upgrade them all together** (stop the old ones, then
  start the new). Instances before 1.7 wait for the per-write notifications that 1.7 no longer
  sends, so they would miss realtime events written through a 1.7 instance.
- A reverse proxy that compresses responses itself should leave slonbase's uncompressed
  answers alone (see [deployment](docs/deployment.md#reverse-proxy)).

## [1.6.0] - 2026-11-02

**Fields and excerpts, as in PocketBase.** The `fields` parameter picks nested paths on every
JSON answer and makes plain-text excerpts of HTML, matching PocketBase's output; JavaScript
`record.set()` takes PocketBase's modifiers.

### Added

- **The `fields` parameter, as in PocketBase**: nested paths (`expand.author.name`,
  `meta.tags.label`), `*` for every key at a level (`*,expand.author.id`), and the
  `:excerpt(max, withEllipsis?)` modifier for plain-text excerpts of HTML
  (`fields=id,title,body:excerpt(200,true)`), on every successful JSON answer (auth, settings,
  collections, batch requests and custom routes too, where only records' top-level keys were
  picked before) and on realtime records. Excerpts match PocketBase's: compared on 5,300
  random HTML documents, and checked in CI against PocketBase's own excerpts of a corpus.
  HTML nested more than 512 elements deep can't be excerpted, as in PocketBase (whose parser
  stops there): the answer is then sent unpicked, and a hostile document costs a bounded
  amount of work.
- **Modifiers in JavaScript `record.set()`**, as in PocketBase: `tags+`, `+tags`, `tags-`,
  `views+`, `views-` and `code:autogenerate`, so `record.get()` sees the result; and
  `set("expand", ...)` / `get("expand")`.
- `$security.randomStringByRegex(pattern)` in JavaScript.

### Changed

- A `fields` parameter that can't be parsed (`body:excerpt(0)`, `id,(title`) gets a 400, as in
  PocketBase, where slonbase ignored it. A request that changes something gets it before the
  change is made (PocketBase makes the change, then answers 400).

### Fixed

- Expanding a relation gives every record an `expand` object, empty when nothing was found,
  and a multiple relation with nothing found no key, as PocketBase answers (`expand` was left
  out, and such relations were `[]`).

## [1.5.0] - 2026-11-02

**Autogenerate patterns, as in PocketBase.** A text field's `autogeneratePattern` takes any
regular expression, and the `field:autogenerate` modifier appends a generated value.

### Added

- **Any regular expression as `autogeneratePattern`**, as in PocketBase: `INV-[0-9]{6}`,
  `(draft|post)_[a-z0-9]{8}`, `\d{4}-\d{4}` and so on, where only `[charset]{n}` patterns
  were accepted before, so collections from PocketBase that use them import as they are. A
  pattern that can produce values the field refuses (too long, not matching its `pattern`) is
  reported when the collection is saved.
- **The `field:autogenerate` modifier** for text fields: the given value followed by one
  generated from the field's pattern, on create and update.

## [1.4.0] - 2026-11-01

**Every PocketBase sign-in provider but one.** Fifteen more OAuth2 providers, so slonbase now has
all of PocketBase's presets except VK.

### Added

- **15 more OAuth2 providers**, for the same presets as PocketBase: Box, Gitea/Forgejo
  (`gitea`), Gitee, Instagram (`instagram2`), Lark, Linear, LiveChat, mailcow, monday.com
  (`monday`), Notion, Planning Center (`planningcenter`), Strava, Trakt, WakaTime and Yandex,
  each with its logo in the dashboard. As with the others, only emails the provider has
  verified link accounts, and accounts the provider reports as inactive can't sign in.

### Fixed

- The PKCE verifier is only sent with the code when the provider uses PKCE (`pkce: false`
  turned off the challenge but still sent the verifier, which some providers refuse).

## [1.3.2] - 2026-11-01

**Every stored secret encrypted.** With an encryption key, OAuth2 client secrets and projects'
database role passwords are now encrypted in the database too, like the settings and the token
signing secret.

### Security

- **Encryption at rest covers every stored secret.** With an encryption key
  (`SLONBASE_ENCRYPTION_KEY`), OAuth2 client secrets in auth collections and the passwords of
  projects' own database roles are now stored encrypted too, besides the settings and the
  token signing secret: a database dump or a backup no longer reveals them. Existing ones are
  encrypted on the next start with the key, `slonbase rekey` covers them, and a restore checks
  that this installation's key opens them.

### Upgrading

- With an encryption key set, the first instance of this version to start encrypts the client
  secrets, which earlier versions can't read: upgrade every instance together (stop them all,
  then start the new version), not one at a time. Without a key, nothing changes.

## [1.3.1] - 2026-11-01

**A snappier dashboard.** Lists you've seen show at once, long pages scroll smoothly, deletes
are immediate, and rarely used pages load only when opened. Nothing changes in the API.

### Changed

- **A snappier dashboard.**
  - **Lists show at once:** a list you've seen shows instantly while the fresh one loads
    (kept in the tab's memory, forgotten when the signed-in account changes).
  - **Long pages scroll smoothly:** record pages go up to 500 rows, and pages over 80 draw only
    the rows in view and around them.
  - **Deletes are immediate:** deleted records go at once, and come back if the server
    refuses; saved values show in the table right away.
  - **Rarely used pages load when opened:** the schema diagram, Logs, Health, the SQL console,
    Projects and the API docs no longer download on the first visit.

## [1.3.0] - 2026-11-01

**Faster everywhere, and encrypted secrets.** Lists are 5 to 8 times faster than in 1.2 and
single records 2.85 times, with the dashboard cached between builds; and the settings and the
token signing secret can be stored encrypted in the database.

### Added

- **The settings and the token signing secret can be encrypted in the database**
  (`SLONBASE_ENCRYPTION_KEY`, `--encryption-key` or `server.encryption_key`: 32 characters,
  AES-256-GCM). A database dump, a read-only role or a backup then reveals neither the
  settings' secrets (SMTP password, S3 keys) nor the secret every token is signed with.
  Existing installations are encrypted on the first start with a key, and tokens stay valid.
  Without the key, or with another one, slonbase refuses to start and says why; a restore the
  key can't read is refused. `slonbase rekey --to <key>` changes the key (or removes it) for the
  main installation and every project. The format is PocketBase's `PB_ENCRYPTION`, so an
  import with the same key brings PocketBase's encrypted settings along. See
  [encrypting the settings](docs/configuration.md#encrypting-the-settings).

### Performance

Lists are 5 to 8 times faster than in 1.2, single records 2.85 times, on the same machine and
data; against PocketBase 0.40.5, lists are 3.3 to 5.3 times faster at about a tenth of its p99
latency ([benchmarks](bench/README.md)). What changed:

- **A fast allocator in the release binaries.** They're static musl builds, whose allocator
  serializes threads on every allocation; the binary now uses mimalloc (programs using slonbase
  as a library keep choosing their own).
- **Prepared statements** for SQL that repeats, once per connection, so PostgreSQL stops
  parsing and planning the same queries. SQL seen once runs as before, so a stream of distinct
  filters can't fill the caches; each connection keeps at most 256, dropped when collections
  change. List pages pass their limit and offset as parameters, so pages share a statement.
- **List totals are kept until data changes.** Counting every matching row was most of a
  list's cost. A total is reused only for the same query and caller, while no data changed
  (from any instance or program: the realtime triggers report every change), and for 5 s at
  most; never for view collections or filters on the time (`@now`, `strftime()`).
- **Lists are written straight from the rows** when no hook, `expand` or `fields` changes them:
  the same bytes, without building a map per record.
- **The dashboard is cached for good** between builds: a repeat visit loads 2 files instead of
  revalidating 33, and the page preloads every module.

### Fixed

- A list hiding emails (`emailVisibility`) no longer moves another field into the email's
  place in each record; the fields keep their order.

### Upgrading

- No migrations. Set the encryption key only once every instance runs this version: older ones
  can't read encrypted settings.

## [1.2.0] - 2026-10-31

**Downloads from the bucket, and six more sign-ins.** File downloads can come straight from S3
through short-lived signed URLs; Bitbucket, Twitch, Spotify, Patreon, Kakao and X join the
OAuth2 providers; and the auth email throttle from 1.1.0 now holds across instances.

### Added

- **Six more OAuth2 providers:** Bitbucket, Twitch, Spotify, Patreon, Kakao and X (`twitter`),
  with their logos in the dashboard. As with the others, only emails the provider verifies are
  used: Spotify's aren't verified and X gives none, so signing up with those needs an email
  from the client (`createData.email`, unverified), as with Microsoft. Bitbucket and X get the
  client's credentials with HTTP Basic authentication, as they require, and X needs PKCE (on by
  default).
- **Downloads straight from S3** (`s3.presignedDownloads`, off by default): file requests are
  answered with a redirect to a short-lived signed URL of the bucket, after slonbase's usual
  checks (protected files, thumbnails, the download hook), so file bytes no longer pass through
  the server. The bucket answers with the same type, disposition and caching. See
  [configuration](docs/configuration.md#file-storage-in-s3).

### Changed

- **The auth email throttle holds across instances.** The verification, password reset, email
  change and OTP request endpoints send each account at most one email of each kind per minute
  (since 1.1.0); the count is now kept in PostgreSQL, so instances sharing a database share it
  too, as they do the rate limits. Before, each instance counted on its own. (If the database
  can't be reached, an instance counts in memory.)

### Upgrading

- Replace the binary or the image; system migration 14 runs on start and adds the
  `_slonbase.mail_sent` table (unlogged). Nothing else changes until you turn on
  `s3.presignedDownloads` or configure a new provider.

## [1.1.0] - 2026-10-30

**A security release.** A review of the whole platform, with a dependency audit and attacks on a
running server, found six problems, all fixed here (the details are in the
[security review](docs/security-review.md#the-platform-110)). The serious one: a client's filter
could read, one yes-or-no answer at a time, records of collections it isn't allowed to list and
emails that accounts hide. Upgrade soon.

### Security

- **Filters only see what the client may list (high).** In a client's filter or sort, relation
  paths (`author.email`) and `@collection` lookups apply the related collection's list rule, and
  a collection only superusers can list contributes nothing. An auth record's `email` counts as
  empty unless the record shows it (`emailVisibility`) or it's the caller's own. Before, a guest
  could read a superusers-only collection's values through a public collection related to it,
  and hidden emails through relations, lookups or a listable auth collection. API rules still
  reach every collection.
- **No more inbox flooding (medium).** The verification, password reset, email change and OTP
  request endpoints send each account at most one email of each kind per minute, whoever asks.
  A repeated OTP request answers with the id of the code already sent.
- **Thumbnails are generated a few at a time (medium)**, so parallel requests for thumbnails of
  large images can't exhaust the memory.
- **Protective headers on every response (low):** `X-Content-Type-Options: nosniff`,
  `X-Frame-Options: SAMEORIGIN` and `Referrer-Policy: strict-origin-when-cross-origin`, and a
  content security policy on the setup page that allows only its own script.
- **The health check no longer describes database errors (low)**, which could name internal
  hosts; the details are in the logs.
- **The request log redacts secrets in URLs (low):** file and download tokens, and OAuth2 codes.
- The dashboard links a URL field's value only when it's an `http` or `https` URL, and reads rich
  text with an inert parser.

### Upgrading

- No system migrations or configuration changes.
- These fixes tighten what clients can do, as [stability](docs/stability.md) allows for
  security fixes. A client filter that reached through a relation or `@collection` into a
  collection the client can't list now matches as if that collection were empty, and filtering
  or sorting by other accounts' hidden emails no longer works. If an app relied on either,
  make the related collection listable to those clients (its list rule), or do the lookup in a
  hook or with a superuser.
- A client asking for the same kind of email again within a minute gets the usual answer, but
  no new email.
- `GET /api/health` answers `Starting.` or `Database unavailable.` without details.

## [1.0.2] - 2026-10-30

**The 1.1.0 security fixes, for 1.0.** For installations staying on 1.0 for now: the fixes of
1.1.0, and nothing else (the details are in the
[security review](docs/security-review.md#the-platform-110)). The serious one: a client's filter
could read, one yes-or-no answer at a time, records of collections it isn't allowed to list and
emails that accounts hide. Upgrade soon, to this release or to 1.1.0.

### Security

- **Filters only see what the client may list (high).** In a client's filter or sort, relation
  paths (`author.email`) and `@collection` lookups apply the related collection's list rule, and
  a collection only superusers can list contributes nothing. An auth record's `email` counts as
  empty unless the record shows it (`emailVisibility`) or it's the caller's own. Before, a guest
  could read a superusers-only collection's values through a public collection related to it,
  and hidden emails through relations, lookups or a listable auth collection. API rules still
  reach every collection.
- **No more inbox flooding (medium).** The verification, password reset, email change and OTP
  request endpoints send each account at most one email of each kind per minute, whoever asks.
  A repeated OTP request answers with the id of the code already sent.
- **Thumbnails are generated a few at a time (medium)**, so parallel requests for thumbnails of
  large images can't exhaust the memory.
- **Protective headers on every response (low):** `X-Content-Type-Options: nosniff`,
  `X-Frame-Options: SAMEORIGIN` and `Referrer-Policy: strict-origin-when-cross-origin`, and a
  content security policy on the setup page that allows only its own script.
- **The health check no longer describes database errors (low)**, which could name internal
  hosts; the details are in the logs.
- **The request log redacts secrets in URLs (low):** file and download tokens, and OAuth2 codes.
- The dashboard links a URL field's value only when it's an `http` or `https` URL, and reads rich
  text with an inert parser.

### Upgrading

- No system migrations or configuration changes.
- These fixes tighten what clients can do, as [stability](docs/stability.md) allows for
  security fixes. A client filter that reached through a relation or `@collection` into a
  collection the client can't list now matches as if that collection were empty, and filtering
  or sorting by other accounts' hidden emails no longer works. If an app relied on either,
  make the related collection listable to those clients (its list rule), or do the lookup in a
  hook or with a superuser.
- A client asking for the same kind of email again within a minute gets the usual answer, but
  no new email.
- `GET /api/health` answers `Starting.` or `Database unavailable.` without details.

## [1.0.1] - 2026-10-28

**A fix for the Projects page.** The main installation's dashboard listed no projects on its
Projects page when there were some. Nothing else changes; upgrade by replacing the binary or
the image.

### Fixed

- **The dashboard's Projects page shows the projects again.** With at least one project, the
  page stayed empty below its description (the project switcher listed them, and the projects
  themselves worked): drawing a project's card failed on a name clash in the page's code.

## [1.0.0] - 2026-10-28

**slonbase 1.0.** A stability promise: within 1.x, upgrades don't break the API, configuration,
your data and backups, hooks, migrations or the Rust library, and a breaking change waits for
2.0. A documentation site with search, at ayeshrodz.github.io/slonbase/docs. And a dashboard
with new typography, set in Ubuntu Sans with one type scale, and micro-interactions throughout.

### Added

- **The stability promise** ([docs/stability.md](docs/stability.md)): what 1.0 keeps compatible
  within 1.x (the API, configuration, your data and backups, hooks and migrations, the Rust
  library, metrics), what it doesn't (the dashboard, message wording, slonbase's own tables),
  how deprecations and upgrades work, and which PostgreSQL versions and releases are supported.
- **A documentation site** at [ayeshrodz.github.io/slonbase/docs](https://ayeshrodz.github.io/slonbase/docs/):
  every guide and the changelog, with a sidebar, an outline that follows the reading position,
  search over every section (`/` or ⌘K), copyable code, and light and dark themes. It's built
  from `docs/` on each change, and the build fails on a link to a missing page or heading.
- **Platform templates ready for their catalogs** ([deploy/README.md](deploy/README.md)): the
  Coolify and Dokploy setups with their logos and catalog entries, the pull requests to submit
  them, and the steps to publish a Railway template.

### Changed

- **The dashboard's typography and motion:** it ships its fonts (Ubuntu Sans and Ubuntu Sans
  Mono, variable), so it looks the same everywhere instead of falling back to the system font,
  and every size and weight comes from one scale. The project's page, the documentation site and
  the setup wizard use the same fonts. Numbers line up in tables and stats, and code shows
  exactly what's typed (no `!=` drawn as `≠`). New
  micro-interactions: pages and titles settle in, cards light up their border under the pointer
  and lift, a pill follows the pointer across the navigation, indicators glide, primary buttons
  catch a sheen, icons respond (arrows nudge, plus turns, refresh spins), switches stretch,
  checkboxes draw their tick, menus and toasts arrive softly (toasts show their time left), and
  the live dot ripples. All of it is off when the system asks for reduced motion.
- The security policy: from 1.0, the previous minor release also gets security fixes, for three
  months after the next one comes out.

### Upgrading

- From 0.8, nothing to do: no system migrations, no configuration changes. Upgrades from older
  releases apply their migrations on start, as before; read their **Upgrading** sections.
- From here on, upgrades within 1.x don't break what [stability](docs/stability.md) covers.

## [0.8.0] - 2026-10-28

**Hardening, and a look at your schema.** Soak testing several instances that share a
database found that realtime delivery fell behind under sustained writes; it now keeps up (from
a p50 of 7 s, and growing, to 13 ms). Backups can be encrypted at rest, large files reach S3 in
parts, and S3 servers with a private CA work. The dashboard gains a Health page, with checks of
the installation and what to do about each, and an interactive diagram of the collections and
their relations.

### Added

- **A schema diagram in the dashboard** (`#/diagram`, in the top navigation and the Data panel):
  every collection with its fields, types, required and unique fields and record counts, and its
  relations as curves from the field to the collection it points at, with one-or-many ends,
  optional relations dashed and cascade deletes marked. Laid out automatically by dependency
  (referenced collections to the left) and arranged by hand from there, kept per browser. Hover or
  select a collection to light up its relations; the side panel lists what it references, what
  references it and its API rules, with links to its records, schema and rules. Find a collection
  or field, show every field, only the keys or only the names, include the system collections,
  tidy up the layout, pan, zoom, fit and use the minimap, and export it as PNG or SVG.
- **A Health page in the dashboard** (`#/health`, linked from the overview): checks of the
  installation with what to do about each (no recent backup, SMTP off, a local application URL,
  rate limits off, several instances with local file storage, errors in the last day, a slow
  database or a full connection pool), this instance's memory and realtime clients, the database's
  round trip, size, migrations and connection pool (with sparklines over the visit), the other
  instances sharing the database, and files and backups. Refreshed every 10 seconds, from the new
  `GET /api/system/status` (superusers).
- **Soak tests** (`soak/`): sustained mixed traffic on several instances sharing a database,
  failing on request errors, missed or late realtime events, memory growth, latency drift or
  leaked connections. CI runs them weekly.
- **A custom CA for S3 and other outbound HTTPS:** `--http-ca-file` (`SLONBASE_HTTP_CA_FILE`,
  `server.http_ca_file`) trusts a PEM bundle's certificates besides the public roots, for S3
  servers (and OAuth2 providers) with certificates from a private CA.
- **Encrypted backups:** with a backups encryption key (`SLONBASE_BACKUPS_ENCRYPTION_KEY`,
  `backups.encryption_key`), new backups are encrypted with AES-256-GCM before they're stored,
  locally or in S3, and anything changed or cut short fails to restore. Restores and uploads
  decrypt them; a plain backup uploaded to such an installation is stored encrypted.
  `slonbase decrypt-backup` turns one back into its zip.
- **Multipart S3 uploads:** files over 64 MiB, such as backups, are uploaded in parts, each
  retried on its own, so large backups no longer depend on one long request and can pass the
  5 GiB a single S3 upload allows. A failed upload is aborted, leaving no parts behind.

### Fixed

- **Realtime keeps up with sustained writes:** events were checked and sent one at a time, so
  under heavy writes their delivery fell seconds behind, and further each minute. They're now
  read in batches and checked several at a time (still in order for each client): in the soak
  test, from a p50 of 7 s (and growing) to 13 ms.

### Upgrading

- No system migrations this time, and nothing changes until you opt in.
- Backup encryption is off unless a key is set. With a key, new and uploaded backups are stored
  encrypted, and existing plain backups still restore. Keep the key somewhere other than the
  server: encrypted backups can't be restored without it.

## [0.7.0] - 2026-10-27

**Parity details, and Sign in with Apple.** Filters and rules gain `strftime()`, reproduced
exactly from SQLite (PocketBase's) and checked against it on tens of thousands of cases, while
staying fast on date fields. Sign in with Apple, with its client secret generator, and OpenID
Connect providers read from a validated `id_token`. Removing a file field deletes its files,
and `logs.maxDataSize` works. Also a new identity: slonbase now describes itself as what it is,
a backend built from scratch for PostgreSQL.

### Added

- **`strftime()` in filters and rules:** `strftime(format, [time-value, modifiers...])`
  formats a date as SQLite's strftime does, as in PocketBase, e.g.
  `strftime('%Y-%m', created) = '2026-01'`. Every format code, time value and modifier
  behaves as in SQLite 3.53 (PocketBase 0.40's), checked against it on tens of thousands of
  cases. On date fields the common formats compile to PostgreSQL's `to_char()`, so filtering
  stays fast. Function calls now take up to 10 arguments.
- **`logs.maxDataSize` works:** log entries whose data is larger keep the attributes that fit
  and are marked with `"__pb_truncated__": true`, as in PocketBase (16 KiB when it's 0), and
  messages are cut at 8,000 bytes. The dashboard's log settings have the field. Values below 0
  are rejected.
- **Sign in with Apple:** an `apple` OAuth2 provider, as in PocketBase. The user comes from
  Apple's `id_token`, verified with Apple's keys, and the name Apple sends only with the first
  redirect is kept for the sign-in (in the database, so it works across instances).
  `POST /api/settings/apple/generate-client-secret` signs the client secret Apple requires, and
  the dashboard's Apple provider form has a generator for it.
- **OpenID Connect sign-in from the `id_token`:** an `oidc` provider without a user info URL
  reads the user from the token response's `id_token`, as in PocketBase. The audience and
  times are checked; the `jwksURL` extra option verifies the signature (RSA or Ed25519 keys)
  and `issuers` limits the accepted issuers. The dashboard's provider form has both.
- **Removing a file field deletes its files:** when a collection change (through the API, an
  import, a hook or a migration) removes a file field, the field's stored files and their
  thumbnails are deleted once the change commits. Before, they stayed in storage with nothing
  referring to them (as they do in PocketBase). Renaming a field keeps them.

### Changed

- **slonbase on its own terms:** the README, the landing page, the docs, the package
  descriptions and `slonbase --help` now describe slonbase as what it is, a backend built from
  scratch for PostgreSQL ("Your whole backend, built on PostgreSQL"), rather than as
  PocketBase's API on another database. Compatibility with the PocketBase SDKs, hooks and
  migrations is unchanged, and is described in one place for people moving an app over.
- The docs are up to date with 0.6 (the design document's roadmap, stale version notes).
- OpenID Connect providers no longer need a user info URL (they're read from the `id_token`
  without one).

### Upgrading

- Two system migrations run on the first start: the `strftime()` functions, and a small
  unlogged table for the name Apple sends with its first redirect. Back up first, as for every
  upgrade.
- Removing a file field from a collection now deletes the field's stored files. Before, they
  were left in storage (with no way to reach them once the column was gone).
- Log entries larger than `logs.maxDataSize` (16 KiB by default) are now truncated, and messages
  are cut at 8,000 bytes.

## [0.6.0] - 2026-10-26

**Realtime for every change, and many instances behind any load balancer.** Realtime events now
come from the database itself, so changes made outside the records API (SQL, auth flows, other
programs) reach subscribers. Several instances can sit behind a plain round-robin load
balancer: realtime subscriptions find the client's instance, and rate limits are shared.
Also `@request.auth` relation paths, Prometheus metrics, and an upgrade test in CI.

### Added

- **Realtime for every change**, not only the records API's: auth flows (verifying an email,
  resetting a password, changing an email), SQL from hooks, migrations and the SQL console, and
  other programs writing to the collection tables now reach subscribers. A trigger on each
  table writes the event in the change's transaction (rolled-back changes still send nothing);
  existing tables get it when the upgrade runs. `SET slonbase.realtime = off` skips events in a
  session, for bulk loads.
- **Several instances behind any load balancer:** realtime no longer needs client affinity
  (a subscribe request reaching another instance than the client's stream is forwarded to it
  through PostgreSQL), and rate limits count the requests of all instances (in an unlogged
  table, only while more than one instance is live).
- **`@request.auth` relation paths** in rules and filters, as in PocketBase:
  `@request.auth.team.owner = @request.auth.id`, `@request.auth.groups.name ?= 'editors'`,
  `@request.auth.team.settings.open = true`. Checked against PocketBase.
- **Prometheus metrics** at `/metrics` on their own address (`--metrics-bind`, off by default):
  requests and durations by route pattern, database pool use, realtime clients and events,
  instances. See [monitoring](https://github.com/ayeshrodz/slonbase/blob/v0.6.0/docs/deployment.md#monitoring).
- CI tests upgrades in place from 0.1.0 and from the previous release.

### Fixed

- Email fields accept what PocketBase accepts: underscores inside domain labels
  (`a@my_host.example`) and a trailing dot; they now reject labels that start or end with `-`,
  empty labels (`x..com`) and IP addresses, as PocketBase does.

### Upgrading

- Two system migrations run on the first start: a realtime trigger on every collection table
  (each table is locked for a moment while it's added) and the tables for instances, forwarded
  subscriptions and shared rate limits. Back up first, as for every upgrade.
- Subscribers may get events they didn't before: for changes made with SQL or by other
  programs, and for auth flows such as email verification. Bulk SQL writes now produce one event
  per row; use `SET slonbase.realtime = off` in that session to skip them.
- With several instances, rate limits now apply across them, so a client gets the configured
  limit once, not once per instance. Raise the limits if you sized them for that.
- Records with an email that PocketBase rejects (such as `a@-x.com`) fail validation the next
  time they're saved.

## [0.5.0] - 2026-10-26

**Every PocketBase hook:** slonbase now has every hook PocketBase's JavaScript declares, from
the realtime and file hooks to `onModel*` and the execute and after-error hooks, plus the
`$dbx` query builder. The compatibility suite runs the same hooks and queries in PocketBase
and slonbase and checks they behave alike (48 tests). The few intended differences are listed
in [the migration guide](https://github.com/ayeshrodz/slonbase/blob/v0.5.0/docs/migrating-from-pocketbase.md).

### Added

- **Realtime, file, settings, backup and terminate hooks**, in JavaScript and Rust:
  `onRealtimeConnectRequest`, `onRealtimeSubscribeRequest`, `onRealtimeMessageSend`,
  `onFileDownloadRequest`, `onFileTokenRequest`, `onSettingsListRequest`,
  `onSettingsUpdateRequest`, `onSettingsReload`, `onBackupCreate`, `onBackupRestore` and
  `onTerminate`, with PocketBase's event properties. The compatibility suite runs the same
  handlers in PocketBase and slonbase. See the [differences](https://github.com/ayeshrodz/slonbase/blob/v0.5.0/docs/migrating-from-pocketbase.md)
  for the two that behave differently.
- **Collection request and batch hooks:** `onCollectionsListRequest`, `onCollectionViewRequest`,
  `onCollectionCreateRequest`, `onCollectionUpdateRequest`, `onCollectionDeleteRequest`,
  `onCollectionsImportRequest` and `onBatchRequest`, also checked against PocketBase.
- **Execute and after-error hooks:** `onRecordCreateExecute`, `onRecordUpdateExecute` and
  `onRecordDeleteExecute` wrap the write itself (code after `e.next()` runs after it, in the
  transaction); `onRecordAfterCreateError`, `onRecordAfterUpdateError` and
  `onRecordAfterDeleteError` report failed writes, including ones a failed batch undid. The
  collection versions (`onCollectionCreateExecute`, `onCollectionAfterCreateError`, ...),
  `onCollectionValidate` and `onBootstrapClear` too. Checked against PocketBase.
- **Generic model hooks:** `onModelCreate`, `onModelUpdate`, `onModelDelete`,
  `onModelValidate`, their `Execute` versions and `onModelAfter{Create,Update,Delete}{Success,Error}`,
  for records and collections (`e.model`), in PocketBase's order. With these, slonbase has every
  hook PocketBase's JavaScript has.
- `toBytes`, `readerToString` and `nullString()`/`nullInt()`/... for `DynamicModel` columns.
- **`$dbx` and the query builder:** `$app.db()` has PocketBase's dbx builder (`select`,
  `where`, joins, `groupBy`, `union`, `insert`, `upsert`, `update`, `delete`, the schema
  methods), `$dbx` its expressions, and `$app.recordQuery(collection)` reads records with it.
  The compatibility suite runs the same queries on PocketBase and slonbase.
- `$app.createBackup(name)` and `$app.restoreBackup(name)`.
- Realtime connections can have an idle and a maximum timeout, set by a connect hook.

### Changed

- `$app.db().newQuery()` runs with the search path `slonbase, public`, so unqualified names
  find collections first, then tables in `public`. A query naming a `public` table that
  shares its name with a collection now reads the collection; qualify it (`public.name`).
- `{{name}}` and `[[name]]` in `newQuery()` SQL are quoted as names, as in PocketBase.

### Fixed

- `$app.db().newQuery()` finds collections by their unqualified names (`SELECT * FROM posts`),
  as the extending guide's example does; they needed the `slonbase.` schema before.
- `$app.db().newQuery()` is faster: it compiled its regular expressions on every call.
- `toString()` of bytes decodes UTF-8 (it read them as Latin-1, so non-ASCII text came out
  garbled).

## [0.4.2] - 2026-10-26

New installations start with rate limits on, and dependencies with security advisories are
updated. Upgrading is recommended.

### Security

- Updated dependencies with RustSec advisories: `rustls` 0.23.46 (four advisories, among them
  a ServerHello downgrade and a decompression bomb), `rustls-webpki` 0.103.16 and
  `webpki-roots`/`webpki-root-certs` 1.0.11 (distrusted certificate authorities), `ctutils`
  0.4.6 (non-constant-time comparisons) and `tokio` 1.54.1 (unsound `AsyncFd` constructors).
- CI runs `cargo audit`, and a release waits for it, so a dependency with a known advisory
  can't be released.

### Changed

- **New installations start with rate limits on**, with the default rules (`*:auth` 2 requests
  per 3 seconds, `*:create` 20 per 5 seconds, `/api/batch` 3 per second, `/api/` 300 per 10
  seconds). PocketBase starts with them off. Existing installations, and imports from
  PocketBase, keep their setting.

## [0.4.1] - 2026-10-23

A security release: everyone on 0.4.0 should upgrade. It also adds ready-made setups for
hosting platforms.

### Security

Fixes from a [review of the authentication code](docs/security-review.md):

- The OTP attempt limit (5 wrong passwords) held for guesses sent one at a time only: guesses
  sent at the same time could all be checked. Each guess now takes an attempt before it's
  checked.
- Behind a proxy that adds its own `X-Forwarded-For` line rather than appending to the client's
  (e.g. HAProxy's `option forwardfor`), a client could choose the IP address that rate limits,
  `superuserIPs` and logs see. Every line of a trusted proxy header is now read.
- OAuth2 avatar downloads only reach public addresses (or the provider's own hosts), so a user
  who can set their avatar URL at the provider can't make slonbase fetch internal addresses.

### Added

- Ready-made setups for hosting platforms ([docs/platforms.md](docs/platforms.md)): a Render
  Blueprint (`render.yaml`, with a Deploy to Render button), Fly.io (`deploy/fly/`), Coolify and
  Dokploy templates (`deploy/coolify/`, `deploy/dokploy/`), and the steps for Railway.

## [0.4.0] - 2026-10-21

**Hooks parity:** most of what `pb_hooks` files use is now there: PocketBase's auth request
hooks, `onRecordEnrich`, `$template`, `$mails` and the record token methods. The
compatibility suite runs the same hooks in PocketBase and slonbase and checks they behave
alike.

### Added

- **Auth request hooks**, from JavaScript and Rust (`Hooks::on_record_auth_request`, ...):
  - `onRecordAuthRequest`: every sign-in, token refresh and impersonation, before the MFA
    check. Handlers can change the record in the response, `e.token` and `e.meta`, or throw.
  - `onRecordAuthWithPasswordRequest`, `onRecordAuthWithOAuth2Request`,
    `onRecordAuthWithOTPRequest` and `onRecordAuthRefreshRequest`, with the hook's data
    (`e.identity`, `e.password`, `e.oauth2User`, `e.createData`, `e.otp`, ...). Handlers can
    find the record their own way.
  - `onRecordRequestOTPRequest`, and the request and confirm hooks of verifications, password
    resets and email changes. A handler that doesn't call `e.next()` skips the action.
- **`onRecordEnrich`**: every record sent to a client (responses, batches, auth responses,
  expanded relations, realtime messages). Handlers can add keys and `hide()` or `unhide()`
  fields.
- **`$template`**: Go's `html/template` language, with `loadFiles()`, `loadString()` and
  `addFuncs()`. Output is HTML-escaped, but not by context like Go's: use `js` and `urlquery`
  inside scripts and URLs.
- **`$mails`**: `sendRecordVerification`, `sendRecordPasswordReset`, `sendRecordChangeEmail`,
  `sendRecordOTP` and `sendRecordAuthAlert`.
- Record token methods: `newVerificationToken()`, `newPasswordResetToken()`,
  `newEmailChangeToken()`, `newFileToken()` and `newStaticAuthToken()`.

### Fixed

- A panic in a JavaScript native function fails that call instead of stopping its runtime.

### Upgrading from 0.3

Install the new version as usual. Nothing changes in the database, and hooks written for 0.3
keep working.

## [0.3.0] - 2026-10-20

**Migrations:** slonbase runs JavaScript migrations, PocketBase's `pb_migrations` included, so
collections can live in code and reach every installation the same way. Each migration runs
in a transaction of its own. See
[docs/migrations.md](https://github.com/ayeshrodz/slonbase/blob/v0.3.0/docs/migrations.md).

### Added

- **JavaScript migrations**, compatible with PocketBase's `pb_migrations`: `*.js` files in the
  migrations directory (`--migrations-dir`, `slonbase_migrations` by default) that call
  `migrate(up, down)`.
  - The server applies pending migrations when it starts. Each runs in a transaction of its
    own, so a failing migration changes nothing; the server then waits, with the error as its
    status, and tries again.
  - Several instances apply each migration once. The applied list is kept in the database and
    in backups.
  - `slonbase migrate up|down|create|collections|history-sync|list`, with `--project`.
  - Automigrate (`--automigrate`, off by default) writes a migration for each collection
    change made in the dashboard or with the collections API.
  - PocketBase's own migration files, automigrate output and snapshots run as they are, and
    `import-pocketbase` marks PocketBase's applied migrations as applied.
- In hooks and migrations, `$app.save()` and `$app.delete()` take collections (and `save` the
  settings) as well as records, with `new Collection({...})`, `new Field({...})` (and
  `TextField`, `NumberField`, ...), `collection.fields.add/addAt/removeById/...`,
  `unmarshal()`, `$app.importCollections()` and `$app.findAllCollections()`.

### Fixed

- Importing collections (the dashboard's import or the API) creates view collections whose
  queries read tables created by the same import, and accepts PocketBase's exports with its
  system collections.

### Upgrading from 0.2

Install the new version as usual (with the package repositories, `apt upgrade` or
`dnf upgrade`).

- System migration 9 (the list of applied JavaScript migrations) applies on start.
- Nothing runs until there are migration files. The packages look for them in
  `/var/lib/slonbase/migrations`, the Docker image in `/data/migrations`, and projects in
  `<projects.path>/<name>/migrations`.
- Backups made with 0.2 restore as before; they leave the list of applied migrations alone.

## [0.2.1] - 2026-10-20

Package repositories, so `apt upgrade` and `dnf upgrade` bring new versions, and a new look
for the dashboard. Nothing changes in the database or the API.

### Added

- Signed apt and dnf repositories at https://ayeshrodz.github.io/slonbase/, updated with every
  release, so `apt upgrade` and `dnf upgrade` install new versions. They have every release since 0.1.0.
  See [installation](https://github.com/ayeshrodz/slonbase/blob/v0.2.1/docs/installation.md#debian-ubuntu-fedora-rhel-packages).

### Changed

- The dashboard has a new palette: cool neutrals, a muted indigo accent for buttons, tabs and
  charts, and muted colors for collections, field types, select values and status. Light and
  dark themes both. A custom dashboard accent from the settings still applies.

## [0.2.0] - 2026-10-19

**Projects:** one slonbase installation can now host several isolated projects. Each project is
a complete installation of its own, with its own collections, superusers, settings, files,
backups, logs and hooks, in a PostgreSQL database of its own on the same server. PocketBase has
no equivalent. See [docs/projects.md](https://github.com/ayeshrodz/slonbase/blob/v0.2.0/docs/projects.md).

### Added

- **Projects**, served under `/p/<name>/`, API and dashboard included. Point the SDK at
  `https://example.com/p/<name>`.
  - Superusers of the main installation create and delete them from the dashboard's Projects
    page, `slonbase project create|list|delete`, or `/api/projects`.
  - Creating a project makes its database, slonbase's system tables in it and its first
    superuser. If a step fails, everything is removed again.
  - Deleting a project asks for its name again and backs it up first. The backup is kept with
    the project's files.
  - Every instance on the main database serves every project, and changes reach all of them
    within moments.
- **Host names** for projects (`shop.example.com`), set from the dashboard,
  `slonbase project hosts` or `PATCH /api/projects/{name}`. Requests are routed by their
  `Host` header.
- **An own database role** for a project, as an option: a PostgreSQL role that owns the
  project's database, which the project connects with and nobody else can.
- Changing a project's hook files restarts only that project.
- A project switcher in the dashboard's top bar, and projects in the command palette.
- `--project <name>` runs `superuser` and `import-pocketbase` on a project, for example to
  move a PocketBase app into one.
- The `projects.path` (`--projects-path`, `SLONBASE_PROJECTS_PATH`) and
  `projects.max_connections` settings.

### Upgrading from 0.1

Install the new version as usual (see
[upgrading](https://github.com/ayeshrodz/slonbase/blob/v0.2.0/docs/installation.md#upgrading)).

- Two system migrations (7 and 8) apply automatically on start.
- The packages and the Docker image keep projects in `/var/lib/slonbase/projects` and
  `/data/projects`.
- Creating projects needs the database role to have `CREATEDB`, and `CREATEROLE` for projects
  with an own role: `ALTER ROLE slonbase CREATEDB CREATEROLE;`. Nothing changes until you
  create a project.
- The main installation opens one more `LISTEN` connection to its database, and each project
  adds up to `projects.max_connections` (4) plus one.

## [0.1.0] - 2026-10-17

The first release: a PocketBase-compatible backend on PostgreSQL.

### Server and setup

- A single static binary for Linux (amd64, arm64). It needs a PostgreSQL 14+ server, which isn't
  bundled.
- A first-run setup wizard (behind a one-time token) that tests the connection, creates the
  system schema and the first superuser, and switches to normal mode without a restart. Headless
  setup is available with `slonbase setup`.
- Configuration through flags, `SLONBASE_*` environment variables and a TOML file. Application
  settings live in the database and apply to every instance at once.
- System migrations run in one transaction under an advisory lock.
- Graceful stops and restarts that don't wait on realtime connections.
- Packages: `.deb` and `.rpm` with a hardened systemd service, a `.tar.gz`, and a multi-arch
  Docker image at `ghcr.io/ayeshrodz/slonbase`. Docker Compose runs it together with PostgreSQL.
- Commands: `serve`, `setup`, `health`, `superuser upsert|update|delete|otp`, and
  `import-pocketbase`.

### Collections and records

- Base, auth and view collections. Each is stored as a real PostgreSQL table in the `slonbase`
  schema.
- Field types: text, editor, number, bool, email, url, date, autodate, select, json, relation,
  file, geoPoint, password.
- Indexes, including unique and partial (`WHERE`) indexes.
- PocketBase's records API: list (paging, sorting, filters, `fields`, `skipTotal`), view,
  create, update, delete.
- API rules and filters with PocketBase's syntax:
  - relation paths, `@request.*`, `@collection.*` lookups, date macros and JSON paths;
  - the `:each`, `:length`, `:lower` and `:isset` modifiers, and `geoDistance`.
- `expand` for relations and back-relations, up to 6 levels, checked against each collection's
  rules.
- Batch requests (`/api/batch`) that run in one transaction.
- Collection import and export, and truncating a collection.
- PocketBase's validation error codes.

### Auth

- Superusers (`_superusers`) and auth collections.
- Sign-in methods:
  - password;
  - OAuth2: Google, GitHub, GitLab, Discord, Microsoft, Facebook and OpenID Connect, with PKCE;
  - one-time passwords;
  - MFA.
- Email verification, password reset and email change, plus impersonation.
- New-device sign-in alerts.

### Realtime and files

- Realtime subscriptions using PocketBase's SSE protocol. They support filters, `expand` and
  `fields`, and respect each subscriber's rules. Events reach every instance through PostgreSQL
  `LISTEN/NOTIFY`.
- File fields:
  - stored on disk or in any S3-compatible bucket;
  - MIME detection from content, and thumbnails;
  - protected files with file tokens;
  - HTTP range requests.

### Operations

- The admin dashboard at `/_/`: collections, the record grid and editor, the schema editor,
  rules, API previews, logs, the SQL console (off by default), and settings.
- Request and error logs, kept in PostgreSQL.
- Rate limits, superuser IP restrictions, and trusted proxy headers.
- Backups (manual and cron, on disk or in S3) and restores that apply to every instance.
- SMTP mail with PocketBase's templates.

### Extending

- JavaScript hooks (`*.pb.js`) with PocketBase's JSVM API:
  - record request, model, validation and after-success hooks;
  - collection and mail hooks;
  - `routerAdd` and `routerUse`, `cronAdd`;
  - `$app`, `$http`, `$security`, `$filesystem` and `$os`.
- A pool of QuickJS runtimes, with a restart when the hook files change.
- A Rust library API: hooks, routes, cron jobs and a record service, for building a custom
  binary.

### Migration

- `slonbase import-pocketbase` imports PocketBase 0.23+ data (a `pb_data` directory or a backup
  zip) without changing it:
  - collections, records, password hashes, superusers and OAuth2 links;
  - files and settings.

### Compatibility and performance

- A compatibility suite runs the official PocketBase JavaScript SDK against slonbase and against
  PocketBase 0.40.5 in CI.
- Benchmarks against PocketBase are in [bench/README.md](https://github.com/ayeshrodz/slonbase/blob/main/bench/README.md).

[Unreleased]: https://github.com/ayeshrodz/slonbase/compare/v2.6.1...HEAD
[2.6.1]: https://github.com/ayeshrodz/slonbase/compare/v2.6.0...v2.6.1
[2.6.0]: https://github.com/ayeshrodz/slonbase/compare/v2.5.3...v2.6.0
[2.5.3]: https://github.com/ayeshrodz/slonbase/compare/v2.5.2...v2.5.3
[2.5.2]: https://github.com/ayeshrodz/slonbase/compare/v2.5.1...v2.5.2
[2.5.1]: https://github.com/ayeshrodz/slonbase/compare/v2.5.0...v2.5.1
[2.5.0]: https://github.com/ayeshrodz/slonbase/compare/v2.4.1...v2.5.0
[2.4.1]: https://github.com/ayeshrodz/slonbase/compare/v2.4.0...v2.4.1
[2.4.0]: https://github.com/ayeshrodz/slonbase/compare/v2.3.1...v2.4.0
[2.3.1]: https://github.com/ayeshrodz/slonbase/compare/v2.3.0...v2.3.1
[2.3.0]: https://github.com/ayeshrodz/slonbase/compare/v2.2.0...v2.3.0
[2.2.0]: https://github.com/ayeshrodz/slonbase/compare/v2.1.3...v2.2.0
[2.1.3]: https://github.com/ayeshrodz/slonbase/compare/v2.1.2...v2.1.3
[2.1.2]: https://github.com/ayeshrodz/slonbase/compare/v2.1.1...v2.1.2
[2.1.1]: https://github.com/ayeshrodz/slonbase/compare/v2.1.0...v2.1.1
[2.1.0]: https://github.com/ayeshrodz/slonbase/compare/v2.0.0...v2.1.0
[2.0.0]: https://github.com/ayeshrodz/slonbase/compare/v1.11.1...v2.0.0
[1.11.1]: https://github.com/ayeshrodz/slonbase/compare/v1.11.0...v1.11.1
[1.11.0]: https://github.com/ayeshrodz/slonbase/compare/v1.10.0...v1.11.0
[1.10.0]: https://github.com/ayeshrodz/slonbase/compare/v1.9.0...v1.10.0
[1.9.0]: https://github.com/ayeshrodz/slonbase/compare/v1.8.0...v1.9.0
[1.8.0]: https://github.com/ayeshrodz/slonbase/compare/v1.7.0...v1.8.0
[1.7.0]: https://github.com/ayeshrodz/slonbase/compare/v1.6.0...v1.7.0
[1.6.0]: https://github.com/ayeshrodz/slonbase/compare/v1.5.0...v1.6.0
[1.5.0]: https://github.com/ayeshrodz/slonbase/compare/v1.4.0...v1.5.0
[1.4.0]: https://github.com/ayeshrodz/slonbase/compare/v1.3.2...v1.4.0
[1.3.2]: https://github.com/ayeshrodz/slonbase/compare/v1.3.1...v1.3.2
[1.3.1]: https://github.com/ayeshrodz/slonbase/compare/v1.3.0...v1.3.1
[1.3.0]: https://github.com/ayeshrodz/slonbase/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/ayeshrodz/slonbase/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/ayeshrodz/slonbase/compare/v1.0.2...v1.1.0
[1.0.2]: https://github.com/ayeshrodz/slonbase/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/ayeshrodz/slonbase/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/ayeshrodz/slonbase/compare/v0.8.0...v1.0.0
[0.8.0]: https://github.com/ayeshrodz/slonbase/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/ayeshrodz/slonbase/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/ayeshrodz/slonbase/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/ayeshrodz/slonbase/compare/v0.4.2...v0.5.0
[0.4.2]: https://github.com/ayeshrodz/slonbase/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/ayeshrodz/slonbase/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/ayeshrodz/slonbase/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/ayeshrodz/slonbase/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/ayeshrodz/slonbase/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ayeshrodz/slonbase/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ayeshrodz/slonbase/releases/tag/v0.1.0

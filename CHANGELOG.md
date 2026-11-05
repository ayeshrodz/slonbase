# Changelog

All notable changes to slonbase are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/): since 1.0, breaking changes wait for a major version
(see [stability](docs/stability.md)). Before 1.0, a minor version (0.x) could contain breaking
changes, listed in its **Upgrading** section.

## [Unreleased]

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

[Unreleased]: https://github.com/ayeshrodz/slonbase/compare/v1.9.0...HEAD
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
[1.1.0]: https://github.com/ayeshrodz/slonbase/compare/v1.0.1...v1.1.0
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

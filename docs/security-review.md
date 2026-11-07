# Security reviews

slonbase's code is reviewed for security before major releases. This page lists what each
review found and fixed, what it checked and found sound, and what it leaves as advice. Every
problem was reproduced, against a running server or with a test, before it was fixed, and the
tests stay in the suite.

To report a problem, see the [security policy](../SECURITY.md).

- [Defaults and degraded modes (2.4.0)](#defaults-and-degraded-modes-240): rate limits when the
  database fails, the lockout default, the SQL console's privileges, and authorization tests.
- [What 1.7 added (1.8.0)](#what-17-added-180): view previews, response compression and the
  outbox reader.
- [The platform (1.1.0)](#the-platform-110): the data API, files, email, the dashboard and
  everything around them.
- [Authentication (0.4.2)](#authentication-042): tokens and every way of signing in.

## Defaults and degraded modes (2.4.0)

Prompted by an outside review of the source, which found the design sound and asked for
safer defaults, explicit degraded behaviour and more evidence for authorization.

### 1. Rate limits weakened when the shared counters failed (medium)

With several instances, rate limits are counted in PostgreSQL. When that failed, each instance
counted on its own, so N instances allowed N times the limit, and only a debug log said so.
Each instance now counts its share of the limit (the limit divided by the live instances,
rounded up), logs a warning once a minute, and Health reports the failure for 15 minutes.
Requests aren't refused outright: most limited endpoints need the database anyway, and refusing
everything would turn a database blip into an outage. `src/guard.rs`, with a unit test of the
shares.

### 2. The sign-in lockout was off by default (medium)

Rate limits count per address, so an account could be guessed at slowly from one address. The
lockout is now on by default (10 failed sign-ins per identity and address in 15 minutes) and
was switched on once for existing installations (migration 19, done in Rust because the
settings may be encrypted). It's keyed on identity and address so that nobody elsewhere can
lock a user out. `tests/guard.rs` (`lockout_is_on_by_default_and_upgraded`).

### 3. The SQL console had slonbase's privileges (low)

It's off by default, read-only by default and for superusers only, but it ran with the role
slonbase connects as. `--sql-console-role` now runs it as a separate, restricted role
(`SET LOCAL ROLE`), and Health warns when slonbase connects as a PostgreSQL superuser.

### 4. Authorization had tests per case, not per path (evidence)

`tests/authz_matrix.rs` now checks that list, view, counts, filters (through relations and
`@collection`), forward and back `expand`, protected files, realtime, update, delete and batch
agree with the same rules, for a guest and two users. `src/filter/compile.rs` compiles
thousands of generated filters full of SQL and filter metacharacters and checks that a client's
value only ever reaches PostgreSQL as a parameter, and 20,000 random inputs that the parser and
compiler never panic.

### Checked and kept

- **A database lookup on every authenticated request.** It's what makes a password change,
  a deletion or "sign out everywhere" end a token on every instance at once; a cache would
  delay that. Kept, and documented in the [threat model](security.md).

## What 1.7 added (1.8.0)

A review, in November 2026, of the three things 1.7 added that handle untrusted input or
shared state: view previews (`POST /api/collections/meta/dry-run-view`, which runs SQL a
superuser types), gzip compression of JSON answers (whose safety against BREACH depends on
which answers it leaves alone), and the outbox reader that replaced per-write notifications.
It found three problems, which are fixed.

### 1. A previewed query could leave locks on a pooled connection (low)

Previews run in a read-only transaction that's rolled back, which undoes everything a
transaction can undo; but a session-level advisory lock (`SELECT pg_advisory_lock(1)`)
outlives the rollback, and the connection went back to the pool holding it. The dashboard
previews a view as it's typed, so a half-written query could leave locks behind for other
requests to wait on. Only a superuser can preview, and only with the SQL console enabled, so
this was no escalation, but a preview shouldn't change anything. The connection is now closed
after every preview, whatever the query did (`dry_run_view_leaves_no_session_state`).

### 2. A custom route's tokens could be compressed (low)

The answers left uncompressed were those on PocketBase's sign-in, refresh, impersonation,
file token, setup and Apple paths, and any answer with a `"token"` key. A JavaScript route
that signs users in and names its token otherwise (`accessToken`, `refresh_token`) would have
been gzipped, and so exposed to BREACH if a page reflects attacker text next to it. Answers
with any of a list of secret keys (`token`, `accessToken`, `access_token`, `refreshToken`,
`refresh_token`, `idToken`, `id_token`, `secret`, `clientSecret`, `client_secret`,
`password`, `apiKey`, `api_key`) are now left alone; the check matches keys, not values, so
a field of type `password` doesn't turn it off.

### 3. A transaction left open made every outbox read slower (low)

The reader asked for events with `xid >= pg_snapshot_xmin(prev)` not visible in its last
snapshot. A transaction left open (a forgotten `BEGIN` in psql, an idle worker) holds the
snapshot's `xmin` back, so each read scanned every event committed since: measured, 17 ms
per read after 100,000 events, growing with the writes, and reads run on every write. Anyone
who could connect to the database could cause it, by accident. The reader now looks up only
the transactions in progress at its last snapshot and the ones that started after it, two
index lookups (0.7–3 ms in the same test); `delivers_late_commits_once` covers a late
commit, and the soak test covers ordering under load.

### Checked and sound

- Previews can't write: `nextval()`, `INSERT` in a CTE and functions that write fail in the
  read-only transaction; `set_config()` is undone by the rollback; `statement_timeout` is set
  for the transaction before the query runs, and a query that changes it only affects
  statements after itself (there are none).
- Request logs keep `token`, `code` and `state` out of logged URLs, so the log list doesn't
  put a secret next to attacker-chosen text (user agents, referers) in a compressed answer.
- Settings and collections send masked secrets (empty SMTP password and S3 secrets, no OAuth2
  client secrets).
- The `changed` notification carries nothing: any role that can `NOTIFY` on the database can
  only make instances read the outbox early.

## The platform (1.1.0)

A review of everything outside the authentication code covered below, done in October 2026
for 1.1.0: a code review, a dependency audit (`cargo audit`: none of the 321 crates has a known
advisory), and attacks on a running server as a guest, a signed-in user and a project's
superuser. It found six problems, which are fixed.

### Scope

- The filter and sort languages, and API rules: what a client's filter can reach, SQL
  injection, and limits (`src/filter/`).
- The records API: field and expand visibility, writes to protected fields, batch requests,
  realtime events and subscriptions (`src/records/`, `src/realtime.rs`).
- Files: serving, protected files, thumbnails, storage keys (`src/records/files.rs`,
  `src/storage.rs`).
- Emails sent on request (verification, password reset, email change, OTP).
- Superuser-only endpoints, settings, backups and their restore, projects and their isolation,
  the setup wizard, the health check, the request log, response headers, and the dashboard's
  handling of record data.

### 1. A client's filter could read data it isn't allowed to see (high)

API rules decide who can list a collection's records. But a filter on a collection the client
*can* list could reach into other collections through relation paths (`author.email`) and
`@collection` lookups (`@collection.secrets.value`), and the related collections' rules
weren't applied. Hidden and password fields were refused, but nothing else was. Each request
answers yes or no (whether records match), so a value can be read one character at a time
(`~ 'a%'`, `~ 'b%'`, ...).

On a test server, a guest read a value from a collection only superusers could list, through a
public collection with a relation to it. An auth collection's emails leaked too, even with
`emailVisibility` off: through relations, `@collection`, and by filtering (or sorting) the auth
collection directly when it's listable.

**Fixed:** in a filter or sort sent by a client (not a superuser), relation paths and
`@collection` lookups only see the records the client could list: the related collection's
list rule is applied, and a collection only superusers can list contributes nothing. An auth
record's `email` counts as empty unless the record shows it (`emailVisibility`) or it's the
caller's own. API rules, which superusers write, still reach every collection, as before.
Test: `filters_only_see_what_the_client_may_list` (`tests/security.rs`).

### 2. The email endpoints could flood an inbox (medium)

`request-password-reset`, `request-verification`, `request-email-change` and `request-otp` each
send an email to an existing account. Only the general rate limit (300 requests per 10 seconds
per client address) applied, so one client could send someone about 30 emails a second, and
several clients more. That harms the person, and the reputation of the server's mail domain.

**Fixed:** each account gets at most one email of each kind per minute from these endpoints,
however many requests, from however many addresses. Requests still get their usual answer, and
a repeated OTP request answers with the id of the code already sent, which keeps working.
Emails sent from hooks or the dashboard aren't limited. Test:
`auth_emails_are_throttled_per_account` (`tests/auth_methods.rs`).

### 3. Parallel thumbnail requests could exhaust the memory (medium)

The first request for an image's thumbnail decodes the image, which can take up to 256 MB
(images are limited to 12,000 pixels a side). Nothing limited how many were decoded at once, so
a few dozen parallel requests for thumbnails of large images could use gigabytes and get the
server killed.

**Fixed:** at most 2 to 4 thumbnails (by the number of CPUs) are generated at once, and a
request that waited first checks whether another one made its thumbnail in the meantime.

### 4. Responses lacked protective headers (low)

API responses had no `X-Content-Type-Options`, and the setup page, whose URL carries the setup
token, had no content security policy.

**Fixed:** every response is sent with `X-Content-Type-Options: nosniff`, `X-Frame-Options:
SAMEORIGIN` and `Referrer-Policy: strict-origin-when-cross-origin`, unless it sets its own. The
setup page allows only its own script (by hash) and nothing from other origins. Test:
`security_headers_and_quiet_health`.

### 5. The health check described database errors to anyone (low)

`GET /api/health` answered, to anyone, with the database error while starting or when the
database was unreachable, which can name internal hosts and ports.

**Fixed:** it says only that slonbase is starting or that the database is unavailable; the
details are in the logs. Test: `security_headers_and_quiet_health`.

### 6. The request log kept secrets from URLs (low)

The request log, which superusers can read, stored full URLs: the tokens of protected file and
backup downloads (`?token=`), and OAuth2 authorization codes (`?code=`). They're short-lived,
but logs are kept for days.

**Fixed:** the values of `token`, `code` and `state` are logged as `REDACTED`. Test:
`logs::tests::secrets_kept_out_of_logged_urls`.

### Also hardened

- The dashboard shows a URL field's value as a link only when it's an `http` or `https` URL
  (the server only accepts those, but a value could be written around the validation).
- The dashboard reads the text of rich text fields with an inert parser, so nothing in them
  loads. The content security policy already kept their scripts from running.

### Checked and sound

- **SQL injection:** values in filters, rules and sorts are always query parameters.
  Identifiers are quoted, and the few things written into SQL (`strftime()` modifiers, units)
  are parsed into fixed values first. Sort fields must be real, visible fields.
- **Limits:** filters can't nest deeper than the parser allows, URLs over the server's limit
  are refused, `perPage` is capped at 1,000, expand depth is limited, JSON bodies are limited to
  2 MB and multipart ones to 128 MB.
- **Record visibility:** `expand` applies the related collection's view rule. Hidden and
  password fields can't be requested with `fields`, filtered or sorted by a client. Realtime
  events are sent only to clients whose rules allow the record, without hidden fields or hidden
  emails. A realtime subscription stays with the credentials that started it.
- **Protected fields:** `verified`, `tokenKey`, `id` and `created` can't be set by a client;
  email changes need a confirmed request and password changes the old password. Changing a
  password signs the record out everywhere. Tokens with `alg: none` or a changed payload are
  refused.
- **Batch requests** go through each record endpoint's own rules, and only record endpoints
  can be called.
- **Files:** names are checked, every storage key is checked for `..`, separators and hidden
  names on every operation (so a superuser's custom id pattern can't reach outside the storage
  directory), and files are served with `nosniff`, a sandboxing content security policy and, for
  protected files, `no-store`. Protected files need a file token and the view rule.
- **Superuser-only endpoints** (settings, collections, SQL, crons, backups, logs, projects,
  impersonation, status) refuse guests and record tokens. Settings never return the SMTP
  password or S3 secret.
- **Backups:** archive entries can't point outside the restore directory, and a restore loads
  tables with `COPY` and recreates them from the restored collections; it never runs SQL from
  the archive.
- **Projects:** a project's tokens don't work on the main installation or other projects, and
  the reverse; paths can't escape `/p/{name}/`. Project databases and roles are created with
  quoted names and a random password.
- **Emails** fill only the application's name and URL and the token or code into templates,
  escaped. OAuth2 sign-in uses only emails the provider says are verified.
- **Dependencies:** `cargo audit` reports no known vulnerability in any of them.

### Advice, not changed

- **View collections and the database role.** A view collection's query is a `SELECT` written by
  a superuser, and it runs with the database role's privileges. With a role that's a PostgreSQL
  superuser, or has `pg_read_server_files` or `pg_signal_backend`, a view can read files on the
  database server or cancel other connections. Give slonbase a role that owns its database and
  nothing more ([deployment](deployment.md#postgresql)), and give projects their own role when
  their superusers aren't the server's operators.
- **Outbound requests from settings.** Superusers choose the S3 endpoint, SMTP host and OAuth2
  provider URLs, and the test buttons connect to them, so a superuser can reach addresses on the
  server's network. A project's superusers can too; if they shouldn't, limit the server's
  outbound network.
- **The email throttle was per instance.** With several instances, an account could get one
  email of a kind per minute from each. Since 1.2, the instances share it through the database,
  as they share the rate limits.

## Authentication (0.4.2)

A review of slonbase's authentication code, done in October 2026 before a wider launch. It
found three problems, which are fixed. This page lists them, what was checked and found sound,
and what's left as advice.


### Scope

- Tokens: signing, verification, types and expiry, and the request extractor (`src/auth.rs`).
- The auth endpoints: password sign-in, refresh, impersonation, email verification, password
  reset, email change, one-time passwords, MFA and OAuth2 (`src/records/auth_api.rs`,
  `otp.rs`, `oauth2_api.rs`, `src/oauth2.rs`).
- The records API's handling of auth fields (`email`, `password`, `verified`, `tokenKey`).
- The setup wizard's token, rate limits, the superuser IP allowlist and client IP resolution
  (`src/setup.rs`, `src/guard.rs`, `src/ip.rs`).

Each problem was reproduced with a test before it was fixed, and the tests stay in the suite.

### 1. The OTP attempt limit could be exceeded with parallel guesses (medium)

An OTP is invalidated after 5 wrong passwords. `auth-with-otp` read the attempt count, checked
the password with bcrypt (tens of milliseconds) and only then counted a wrong one. Guesses sent
at the same time all passed the check before any of them was counted. In the test, 40
simultaneous guesses had 19 checked against one OTP.

Rate limits are off by default, so an attacker who knows a user's email could try far more than
5 codes per OTP, and request up to 9 OTPs at a time.

**Fixed:** each guess takes an attempt in the same statement that checks the limit
(`UPDATE ... WHERE attempts < 5 RETURNING ...`), before the password is checked. Test:
`otp_attempts_hold_under_concurrent_guesses`.

### 2. A client could choose its own IP address behind some proxies (medium)

With a trusted proxy header configured (`trustedProxy.headers`, e.g. `X-Forwarded-For`),
slonbase read only the header's first line. Most proxies append the client's address to the
existing header, but some add a header line of their own instead (HAProxy's `option
forwardfor`, for example). A client that sent its own `X-Forwarded-For` then had its line read,
and could pick any address. That address is what rate limits, the superuser IP allowlist
(`superuserIPs`), logs and auth alerts use.

**Fixed:** every line of the header is read, in order, and the rightmost (or, with
`useLeftmostIP`, leftmost) address across all of them is used. Test: `ip::tests::proxy_headers`.

### 3. Avatar downloads could reach internal addresses (low)

When OAuth2 sign-in creates a record and the collection maps the avatar to a file field,
slonbase downloads the provider's avatar URL. Some identity providers let users set that URL
(e.g. OpenID Connect's `picture` claim). A user could point it at an internal address, such as
a cloud metadata service or an internal HTTP service. The response would be saved as their
avatar, which they can then download.

**Fixed:** avatars are only downloaded from public addresses. Loopback, private, link-local,
shared, reserved and multicast ranges are refused, and so are IPv4-mapped and NAT64 forms of
them. slonbase connects to the address it checked, so a second DNS lookup can't change it.
Redirects are followed by hand (at most 3) and each one is checked the same way. The
provider's own hosts (from its configured URLs) are still allowed, since a superuser chose them
and a self-hosted provider may be on a private network. Tests:
`oauth2::tests::avatars_only_from_public_addresses`, `ip::tests::public_addresses`.

### Checked and sound

- **Tokens** are HS256 JWTs. The key is derived from a 32-byte random secret, plus the
  collection, the token type and the record's `tokenKey`. So a token of one type, collection or
  record can't be used as another, and the `alg` header can't be swapped. Signatures are
  compared in constant time, and expiry is checked. Changing a password or email rotates the
  `tokenKey`, which signs the record out everywhere and makes reset and email change tokens
  single-use.
- **Password sign-in** compares against a real bcrypt hash even when no record matches, so
  response times don't reveal which emails exist. The collection's `authRule` is checked after
  the password. An empty stored hash never matches.
- **Flows started by email** (verification, password reset, OTP) answer the same way whether or
  not the email exists, and send the email in the background. Tokens sent by email are tied to
  the email they were sent to.
- **MFA** sessions are bound to the record and collection, expire, need a different second
  method, and are single-use even when two requests race.
- **OAuth2** only uses emails the provider marks as verified. Microsoft's `mail` isn't used,
  since Microsoft doesn't verify it. An unverified record that matches a provider's verified
  email gets a new password and `tokenKey` before it's marked verified, so whoever created it
  can't keep access. Linking an account that's already linked to another record fails. PKCE is
  on by default.
- **Auth fields in the records API:** only superusers and managers (`manageRule`) can set
  `verified` or change another record's email or password. A record changing its own password
  must send `oldPassword`. The email index is case-insensitive.
- **The setup wizard** needs its token (at least 32 characters, compared in constant time) for
  every step, including the database test, and closes once setup is done.
- **Random values** (secrets, ids, OTPs, token keys) come from a CSPRNG.

### Advice, not changed

- **Turn rate limits on.** They were off by default, as in PocketBase, so nothing limited
  password guessing until a superuser enabled them. Since 0.4.2, new installations start with
  them on (2 sign-in attempts per 3 seconds per client and collection). Older installations keep
  their setting: turn them on in Settings → Rate limits.
- **bcrypt reads only the first 72 bytes of a password.** The default maximum of 71 characters
  stays under that for ASCII, but a password of non-ASCII characters, or a higher maximum, can
  go over it. The bytes past 72 are then ignored.
- **A self-hosted GitLab** with email confirmation turned off reports unconfirmed emails as the
  user's email. Use such an instance as a provider only if it confirms emails.
- **Concurrent password resets with the same token** can both succeed (the last one wins). That
  needs the token, which already allows the reset.

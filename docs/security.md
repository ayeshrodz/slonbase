# Threat model and hardening

What slonbase protects, what it trusts, and how to check a deployment. It's written for people
deploying slonbase and for anyone reviewing its security: if you find a way around anything
below, please [report it privately](https://github.com/ayeshrodz/slonbase/security/policy).

## What slonbase trusts

| Who or what | Trusted with | Not trusted with |
|---|---|---|
| **Clients** (browsers, apps, anyone on the network) | Nothing: every request is checked | Filters, sorts, `expand`, file names, uploads, headers, tokens |
| **App users** (records of auth collections) | What the collection's API rules allow them | Anything else, including other users' records and hidden fields |
| **Superusers** | The installation's data, schema and settings, within their role (owner, admin, developer, viewer) | Other projects' data, unless they're superusers there too |
| **Hooks and migrations** (JavaScript files on disk) | Everything the server can do | (They're your code: deploy them like code) |
| **The PostgreSQL role** slonbase connects with | Its database | Should not be a PostgreSQL superuser |
| **Your reverse proxy** | The client's address, but only when `trustedProxy` is set (and, with `trustedProxy.peers`, only on connections from the listed addresses) | Headers, when `trustedProxy` isn't set or the connection isn't from a listed proxy |

## The boundaries, and what enforces them

**Authorization.** Each collection has list, view, create, update and delete rules in one filter
language, compiled to SQL. The same compiled rule applies on every path a record can take:

- list and view, and the counts in list responses;
- filters and sorts sent by a client: they can only reach records and fields the client could
  list, through relations and `@collection` too;
- `expand`, forward and back (`*_via_*`);
- protected files (served with a short-lived file token, checked against the view rule);
- realtime events (each subscriber gets only the records it may view);
- update, delete and batch requests.

`tests/authz_matrix.rs` checks all of these paths against one set of rules on every change, so a
path that disagrees fails CI.

**Injection.** A client's values reach PostgreSQL only as parameters, never as SQL text. Field
and collection names are checked against the schema and quoted. A generated test compiles
thousands of filters full of SQL and filter metacharacters and checks that none reaches the SQL
(`src/filter/compile.rs`).

**Authentication.** Tokens are HMAC-SHA256 signed with a key made from the installation's secret,
the collection, the token's kind and the record's own token key, and every request checks the
record in the database. So changing a password, deleting the record or
`POST .../records/{id}/revoke-tokens` ("sign out everywhere") ends all of its tokens at once, on
every instance. That database lookup is deliberate: a cache would delay revocation.
Passwords are hashed with bcrypt; OTPs and MFA are available; an encryption key seals the
signing secret and the settings' secrets at rest.

**Abuse.** Rate limits (on for new installations) count per address, shared by the instances
through PostgreSQL. If the shared count fails, each instance counts its share of the limit until
it works again, so the total still holds, and Health shows it. The sign-in lockout (on by
default since 2.4) counts failed password sign-ins per identity and address. Auth emails are
throttled per account.

**Transport and browsers.** slonbase doesn't terminate TLS: run it behind a proxy that does, and
set an https application URL (Health warns otherwise). Responses carry protective headers; CORS
allows any origin by default (as PocketBase), and can be narrowed.

**Superuser tools.** The SQL console is off by default; when on, it only reads unless writes are
allowed too, and it can run as a separate, restricted PostgreSQL role
(`--sql-console-role`). Hooks run with the server's privileges, by design. A dashboard cron job
of type *Run SQL* is the console on a schedule: owners only, only with the console on, writes only
when the console allows them, as the console's role and with its limits.

**Outbound calls.** The OAuth2 avatar download and the dashboard's *Call a URL* cron jobs only
reach public addresses: every address a name resolves to must be public (no loopback, private,
link-local or metadata address), the connection goes to the addresses just checked, and each
redirect is checked the same way (the same goes for a cron job's failure webhook). Only admins and owners can make such a job; header values are
stored with the other private settings and shown hidden.

## Configuration warnings

Some settings are valid and documented, and are only safe with something more. slonbase lists
them on the dashboard's Health page, in `GET /api/settings/warnings` and in the log at start. They
change nothing by themselves:

- **`trustedProxy.headers` without `trustedProxy.peers`:** any client can send those headers and
  choose the address that rate limits and sign-in lockouts see. Name your proxies in
  `trustedProxy.peers`, unless only the proxy can reach slonbase.
- **An OpenID Connect provider without `extra.jwksURL`:** the `id_token` is trusted because it
  comes straight from the provider's token endpoint, as OpenID Connect allows when that endpoint
  is reached over TLS, and its signature is not checked. Use an `https://` `tokenURL` (slonbase
  does not require it by default, and does not stop the provider's redirects from leaving TLS, so
  make sure the token endpoint and any redirect it answers with use HTTPS), and set `extra.jwksURL` to check the signature too. With
  `extra.requireSignature` set to `true`, slonbase refuses the provider unless `jwksURL` is set,
  and requires `tokenURL`, `jwksURL` and `userInfoURL` to be `https://`. A provider with a
  `userInfoURL` takes the user from that endpoint, **not** from the signed token, so leave
  `userInfoURL` empty to rely on the signature for sign-in. With `requireSignature`, the token,
  `jwksURL` and `userInfoURL` requests also refuse a redirect to a URL that isn't `https://`.
- **An empty `manageRule`, `updateRule` or `deleteRule` on a collection that holds accounts:**
  as in PocketBase, an empty rule allows everyone, guests included (on an auth collection, an
  empty `manageRule` lets a guest create records with `verified = true`). A `null` rule keeps it
  to superusers.

- **Hook code without a time or memory limit:** with hook files present and both
  `hooks.timeout_seconds` and `hooks.memory_limit_mb` at 0, a hook that loops or allocates
  without end is not stopped. New installations set 60 seconds and 128 MB, but an installation
  whose database URL is fixed by a flag or an environment variable has no setup-written
  configuration file, so it keeps 0 until you set them (in the file, or with `--hooks-timeout`
  and `--hooks-memory`). slonbase lists it on the Health page and never changes the limits.

- **A stored template or automation that no longer passes its checks:** validation runs when
  one is saved, so a rule that became stricter (or a collection or field an automation uses that
  was renamed) only shows when it runs. slonbase checks them again as the installation starts and
  when the settings or the collections change, and lists what no longer validates, with the
  fields that fail, on the Health page. It never changes or switches anything off.

Two rule semantics catch people out, and slonbase keeps PocketBase's for compatibility:

- An `updateRule` is checked against the record **before** the update, with the request's body
  as `@request.body`. A rule that must constrain the **new** value has to say so, for example
  `@request.auth.id = id && @request.body.role:isset = false`.
- An empty rule is open to everyone; leave a rule `null` to keep it to superusers.

## Known limits

Past issues are listed, with their advisories, in the [security reviews](security-review.md) and in
[SECURITY.md](https://github.com/ayeshrodz/slonbase/blob/main/SECURITY.md#published-advisories).

Found by reviews, and what they mean in practice:

- **Trusted proxy headers are believed from any connection unless you list your proxies.** Set
  `trustedProxy.peers`; without it, slonbase believes the headers it names from whoever connects,
  and Health warns.
- **OIDC providers without a key set** (`jwksURL`) are trusted over HTTPS for the ID token's
  signature and issuer. Use a provider with a key set.
- **Backups aren't encrypted unless you set a key** (Health warns). Their files are read from the
  storage while the database snapshot is taken, so a file uploaded or deleted during a backup can
  be missing from it or extra in it; the database is consistent, the file set is that of a moment
  just after.
- **A restore commits the database, then swaps the files in.** The files are staged beside the
  storage first, so only two renames on one file system follow the commit; a crash in between
  leaves the new database with the old files, and the backup can be restored again.
- **A first OAuth2 sign-in creates the record, marks it verified and links the provider in
  separate steps.** If linking fails (two sign-ins of the same new account racing), the record
  made for it is removed again; a failure of the later steps (marking it verified) leaves a
  record the next sign-in with the same verified email picks up.
- **CORS allows every origin by default** (`*`, as PocketBase does). That is safe here because
  tokens travel in the `Authorization` header, never in cookies, so a page on another origin
  can't use a visitor's session. Limit it in Settings, Security, to the origins of your apps if
  you want browsers to refuse the rest.
- **No query time limit by default.** A view that is slow to read, or a heavy filter, runs until
  it ends and holds a pooled connection meanwhile. Set `database.statement_timeout` (seconds) to
  end such queries; backups and restores are exempt. It's off by default because a limit that is
  too low would cut long migrations and index builds.
- **Projects need `CREATEDB`** on the main installation's PostgreSQL role. Connect existing
  databases instead (`databaseUrl`) if you want a role without it.

## What slonbase doesn't protect against

- A compromised server, configuration file or PostgreSQL role: they hold the keys.
- Rules you write: `""` lets anyone in. Review them, and use the rules' dry run in the
  dashboard.
- Denial of service by sheer volume: rate limits slow a single address, not a botnet. Put a
  proxy or a CDN in front for that.
- Your hooks' code.

## Checking a deployment

1. Open **Health** in the dashboard. Its security checks flag a superuser database role, rate
   limits or the lockout turned off, an application URL without https, and shared rate limits
   that failed recently.
2. Go through the [security checklist](deployment.md#security-checklist).
3. Keep slonbase up to date: security fixes ship as patch releases (see
   [SECURITY.md](https://github.com/ayeshrodz/slonbase/blob/main/SECURITY.md)).

## Reviews so far

The [security reviews](security-review.md) list what was checked, what was found and fixed, and
the tests that keep each fix in place. Independent reviews are welcome, and credited.

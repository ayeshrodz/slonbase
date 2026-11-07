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
| **Your reverse proxy** | The client's address, but only when `trustedProxy` is set | Headers, when `trustedProxy` isn't set |

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
(`--sql-console-role`). Hooks run with the server's privileges, by design.

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

# API

slonbase serves a REST API for your collections, auth and files, and a realtime stream of their
changes. The dashboard's API preview shows the endpoints of each of your collections, with
examples. Use any HTTP client, or the PocketBase [JavaScript](https://github.com/pocketbase/js-sdk)
and [Dart](https://github.com/pocketbase/dart-sdk) SDKs, which work with slonbase's API (a
[test suite](../compat/) checks them in CI):

```js
import PocketBase from "pocketbase";

const pb = new PocketBase("https://app.example.com");
await pb.collection("users").authWithPassword("ann@example.com", "secret");
const posts = await pb.collection("posts").getList(1, 20, { filter: 'status = "live"', expand: "author" });
pb.collection("posts").subscribe("*", (e) => console.log(e.action, e.record));
```

## Endpoints

| Endpoint | Description |
|---|---|
| `POST /api/collections/{collection}/auth-with-password` | Sign in (`{"identity", "password"}`), returns `{token, record}`; use `_superusers` for superusers |
| `POST /api/collections/{collection}/auth-with-oauth2` | Sign in with an OAuth2 provider (`{"provider", "code", "codeVerifier", "redirectURL", "createData"}`) |
| `POST /api/collections/{collection}/request-otp`, `auth-with-otp` | Sign in with a one-time password sent by email |
| `GET/POST /api/oauth2-redirect` | OAuth2 redirect for the SDKs' popup flow (`authWithOAuth2({provider})`) |
| `GET /api/collections/{collection}/records/{id}/external-auths`, `DELETE .../{provider}` | Linked OAuth2 accounts |
| `POST /api/collections/{collection}/auth-refresh` | A new token for a valid one |
| `GET /api/collections/{collection}/auth-methods` | Available sign-in methods |
| `POST /api/collections/{collection}/request-verification`, `confirm-verification` | Email verification |
| `POST /api/collections/{collection}/request-password-reset`, `confirm-password-reset` | Password reset by email |
| `POST /api/collections/{collection}/request-email-change`, `confirm-email-change` | Email change (signed in) |
| `POST /api/collections/{collection}/impersonate/{id}` | A non-refreshable token for any record (superusers) |
| `GET/PATCH /api/settings` | Settings (superusers): app, SMTP, S3, backups, logs, batch, rate limits, trusted proxy, superuser IPs |
| `POST /api/settings/test/email` | Send a test email of a template |
| `POST /api/settings/test/s3` | Check the S3 settings by writing, reading and deleting a test file |
| `POST /api/settings/apple/generate-client-secret` | Sign in with Apple's client secret (`{"clientId", "teamId", "keyId", "privateKey", "duration"}`), superusers |
| `GET /api/files/{collection}/{record}/{filename}` | Download a file (`thumb=100x100`, `download=1`, `token=` for protected files) |
| `POST /api/files/token` | A short-lived token for protected files |
| `GET /api/realtime`, `POST /api/realtime` | Realtime: an SSE stream, and setting its subscriptions (`posts/*`, `posts/{id}`) |
| `GET/POST /api/collections`, `GET/PATCH/DELETE /api/collections/{idOrName}` | Manage collections (superusers) |
| `PUT /api/collections/import` | Create, update (and with `deleteMissing`, delete) collections in one transaction |
| `DELETE /api/collections/{collection}/truncate` | Delete every record of a collection |
| `GET /api/collections/meta/scaffolds`, `meta/oauth2-providers` | Defaults and providers for the dashboard |
| `POST /api/sql` | The SQL console (when enabled) |
| `GET/DELETE /api/logs`, `GET /api/logs/{id}`, `GET /api/logs/stats` | Logs (filter, sort, per-hour stats), superusers |
| `GET/POST /api/backups`, `POST /api/backups/upload`, `GET/DELETE /api/backups/{key}`, `POST /api/backups/{key}/restore` | Backups (download with a superuser file token) |
| `GET /api/crons`, `POST /api/crons/{id}` | Scheduled jobs (the automatic backup) |
| `POST /api/batch` | Several record creates, updates, upserts (`PUT`) and deletes in one transaction (enable `batch` in the settings) |
| `GET/POST /api/collections/{collection}/records` | List (`page`, `perPage`, `sort`, `filter`, `expand`, `fields`, `skipTotal`) and create records |
| `GET/PATCH/DELETE /api/collections/{collection}/records/{id}` | View, update and delete a record |
| `GET /api/health` | Health check |
| `GET/POST /api/projects`, `GET/PATCH/DELETE /api/projects/{name}` | Projects (superusers of the main installation; see [projects](projects.md)) |

Send the token as `Authorization: Bearer <token>` (or just the token).

```sh
# create a collection anyone can read and add to
curl -X POST localhost:8090/api/collections -H "Authorization: $TOKEN" -H 'content-type: application/json' \
  -d '{"name": "todos", "fields": [{"name": "text", "type": "text", "required": true},
       {"name": "done", "type": "bool"}], "listRule": "", "createRule": ""}'

curl -X POST localhost:8090/api/collections/todos/records -H 'content-type: application/json' -d '{"text": "buy milk"}'
curl "localhost:8090/api/collections/todos/records?filter=done=false&sort=-text"
```

## Collections

Collections are `base`, `auth` or `view`. Each base and auth collection is a real PostgreSQL table
in the `slonbase` schema (`TEXT`, `DOUBLE PRECISION`, `BOOLEAN`, `TIMESTAMPTZ`, `JSONB` columns),
so other tools can query it directly.

**Field types:** `text`, `editor`, `number`, `bool`, `email`, `url`, `date`, `autodate`, `select`,
`json`, `relation`, `file`, `geoPoint`, and `password` (auth collections).

**Auth collections** add the `password`, `tokenKey`, `email`, `emailVisibility` and `verified`
system fields, and auth options: `authRule`, `manageRule`, `passwordAuth`, `oauth2`,
`otp`, `mfa`, `authAlert`, token durations, and email templates. Users sign up by creating a
record with `password` and `passwordConfirm`, or through OAuth2.

**View collections** are read-only records from a SQL query, e.g.
`SELECT author AS id, count(*) AS posts FROM posts GROUP BY author`. Collections are tables named
after their collection. A view's fields come from the query's columns, and list/view rules,
filters, sorting and expand work as usual. The query runs with the database role's privileges,
and a `SELECT` can call any function the role may use, so creating or changing a view requires
the SQL console to be enabled (`--sql-console`). Existing views keep working without it.

**System collections:** superusers are records of the `_superusers` auth collection, and linked
OAuth2 accounts are records of `_externalAuths`, so both are managed with the
records API. System collections can't be renamed or deleted, and the last superuser can't be
deleted.

**Indexes** are given as `CREATE [UNIQUE] INDEX` statements, including partial
indexes with a `WHERE` clause.

## Sign-in

- **Password:** `auth-with-password`, with the identity fields set in `passwordAuth`.
- **OAuth2:** Google, Apple, GitHub, GitLab, Discord, Microsoft, Facebook, and any OpenID Connect
  provider (`oidc`, `oidc2`, `oidc3` with your URLs), with PKCE. An OpenID Connect provider
  without a user info URL is read from its validated `id_token`; set `extra.jwksURL` to verify
  its signature and `extra.issuers` to accept only some issuers. Only emails the provider has
  verified are used to find existing records. Client secrets are never returned by the API.
- **OTP:** a numeric code sent with the `otp.emailTemplate` (`{OTP}`, `{OTP_ID}`). It's
  single-use, it expires, and it's invalidated after 5 wrong attempts.
- **MFA:** with `mfa.enabled`, the first sign-in answers `401 {"mfaId"}`, and a sign-in with
  another method plus that `mfaId` returns the token. `mfa.rule` limits MFA to some records.

Emails (verification, password reset, email change, OTP, new-device alerts) are sent through the
SMTP server in the settings. While SMTP is disabled, they're written to the log.

## Filters and API rules

Filters and rules share one small expression language:

- operators: `=`, `!=`, `>`, `>=`, `<`, `<=`, `~` (contains), `!~`, and the `?`-prefixed "any of"
  variants for multi-valued fields;
- `&&`, `||` and parentheses;
- relation paths (`author.org.name`, `tags.name ?= 'x'`);
- `@request.*`: `@request.auth.*` (the signed-in record's fields, and relation paths from it:
  `@request.auth.team.owner = @request.auth.id`, `@request.auth.groups.name ?= 'editors'`),
  `@request.body.*`,
  `@request.query.*`, `@request.headers.*` and `@request.method`;
- date macros such as `@now` and `@todayStart`;
- JSON paths (`meta.key`), and the `:length`, `:lower`, `:each` and `:isset` modifiers;
- `@collection.name[:alias].field` to look at other collections, e.g.
  `@collection.members.post = id && @collection.members.user = @request.auth.id` (true when some
  row of `members` matches; each alias is a separate row, up to 6 per expression);
- `geoDistance(lonA, latA, lonB, latB)` in kilometers, e.g.
  `geoDistance(loc.lon, loc.lat, 23.32, 42.69) < 25`;
- `strftime(format, [time, modifiers...])`, which formats a date as
  [SQLite's strftime](https://sqlite.org/lang_datefunc.html) does, with its format codes and
  modifiers, e.g. `strftime('%Y-%m', created) = '2026-01'` or
  `strftime('%w', due, '+1 day') = '0'`. The time is a date, a field or a number (a Julian day,
  or Unix time with `'unixepoch'`), and the current time if left out. It returns `''` when the
  time or a modifier isn't valid.

A rule of `null` allows superusers only, and `""` allows anyone. Filters are compiled to
parameterized SQL, so PostgreSQL's planner and your indexes do the work.

**Expand** loads related records into each record's `expand` key. It follows relation fields
(`expand=author.org,editors`) and back-relations (`expand=comments_via_post`), up to 6 levels,
and checks each record against its collection's view rule.

## Files

Files are uploaded with `multipart/form-data` on create and update:

- `avatar` replaces a field's files, `files+` appends, and `files-` (or a list of the names to
  keep) removes.
- Other fields are sent as form values, or as JSON in `@jsonPayload`.
- Types are detected from the content (never the name) and checked against `mimeTypes`.
- Thumbnails (`100x100`, `0x300`, `200x100t`/`b`/`f`) are generated on first request.
- Downloads support byte ranges (`Range`), so audio and video can be streamed.
- Protected files need a file token (`POST /api/files/token`, `?token=`).
- Deleting a record deletes its files, and removing a file field from a collection deletes that
  field's files.

Files are stored under `--storage-path`, or in an S3 bucket
([configuration](configuration.md#file-storage-in-s3)).

## Realtime

Realtime is a Server-Sent Events stream (`GET /api/realtime`) whose subscriptions a client sets
with `POST /api/realtime`; with the SDK, `pb.collection("posts").subscribe("*", ...)`.
Topics are `posts/*` and `posts/{id}`, optionally with `filter`, `expand` and `fields` options.
Each subscriber only receives events its list or view rule allows. Changes are announced through
PostgreSQL `LISTEN/NOTIFY`, so they reach the subscribers of every instance sharing the database
(see [several instances](deployment.md#several-instances)).

Every change of a collection table is announced, whatever makes it: the records API, auth flows
(verifying an email, resetting a password), hooks and migrations writing SQL, the SQL console,
or other programs writing to the tables. A trigger on each table writes the event in the
change's transaction, so rolled-back changes are never announced. A session can skip events for
a bulk load with `SET slonbase.realtime = off`.

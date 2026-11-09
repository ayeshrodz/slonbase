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
| `POST /api/collections/{collection}/records/{id}/revoke-tokens` | Signs a record out everywhere: gives it a new token key, so every token it holds stops working on every instance. The record itself, or a superuser (a superuser's own, or by another superuser) |
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
| `POST /api/collections/meta/dry-run-view` | A view query's fields and first rows, without saving it (`{"query"}`, needs the SQL console) |
| `GET /api/sql` | Whether the SQL console is on, whether it may write, and its limits |
| `POST /api/sql` | The SQL console (when enabled): `{"query", "maxRows"?}` |
| `GET/DELETE /api/logs`, `GET /api/logs/{id}`, `GET /api/logs/stats` | Logs (filter, sort, per-hour stats), superusers |
| `GET/POST /api/backups`, `POST /api/backups/upload`, `GET/DELETE /api/backups/{key}`, `POST /api/backups/{key}/restore` | Backups (download with a superuser file token) |
| `GET /api/crons`, `POST /api/crons/{id}` | Scheduled jobs (hooks', the automatic backup and the ones made in the dashboard), with their schedule in words, next run and last run; the second runs one now |
| `POST /api/crons`, `PATCH /api/crons/{id}`, `DELETE /api/crons/{id}` | Make, change and delete a cron job of the dashboard: `{name, expression, type, enabled, config}` (`type` `cleanup`: `config` has `collection`, `filter`, `olderThanDays`, `field`, `maxRows`; `type` `http`: `url`, `method`, `headers`, `body`, `timeoutSeconds`, `expectStatus`, public addresses only, header values come back as `••••••••` and the mask keeps the saved value; `type` `mail`: `to`, `subject`, `body`, needs SMTP on; `type` `sql`: `statement`, owners only and only with the SQL console on, run under the console's policy). Admins and owners; their ids start with `user:` |
| `POST /api/cron-preview` | Checks a job without saving it: its schedule in words, the next runs and, for a cleanup, how many records match now |
| `GET /api/jobs/runs` | What backups and cron jobs did (`kind`, `job`, `status`, `limit`), superusers |
| `GET /api/settings/storage`, `POST /api/settings/storage/clear-thumbs` | Where files are, the free space and what uploads take; delete generated thumbnails |
| `POST /api/settings/rate-limits/explain` | Which rate-limit rule a request meets (`{method, path, audience}`), superusers |
| `POST /api/settings/mail/preview` | An email as it would be sent, with sample values (`{collection, template}` and/or `{subject, body, layout}`) |
| `GET/POST /api/branding/assets`, `PATCH/DELETE /api/branding/assets/{id}` | The installation's images (superusers; upload as multipart `file`) |
| `GET /api/branding/assets/{id}` | An image, for anyone (sign-in pages and emails show them) |
| `GET /api/team`, `GET /api/team/me`, `PATCH /api/team/members/{id}` | Superusers and their roles (owners change roles); `GET /api/team` also lists `capabilities`, which roles may do what, as the rules below enforce it |
| `POST /api/team/keys`, `DELETE /api/team/keys/{id}` | API keys (the key is in the answer, once) |
| `POST /api/batch` | Several record creates, updates, upserts (`PUT`) and deletes in one transaction (enable `batch` in the settings) |
| `GET/POST /api/collections/{collection}/records` | List (`page`, `perPage`, `sort`, `filter`, `expand`, `fields`, `skipTotal`) and create records |
| `GET/PATCH/DELETE /api/collections/{collection}/records/{id}` | View, update and delete a record |
| `GET /api/health` | Health check |
| `GET /api/system/status` | The installation's status and checks, for the dashboard's Health page (superusers) |
| `GET/POST /api/projects`, `GET/PATCH/DELETE /api/projects/{name}` | Projects (superusers of the main installation; see [projects](projects.md)) |
| `GET /api/projects/overview`, `GET/PATCH /api/projects/{name}/settings`, `POST /api/projects/{name}/restart` | The command center: every installation's numbers, a project's settings, a restart (see [projects](projects.md#the-command-center)) |

Send the token as `Authorization: Bearer <token>` (or just the token). An API key
(`sb_…`, Settings → Team) goes the same way: it acts for the superuser who made it, with its own
role.

### Roles

Each superuser of an installation has a role (Settings → Team); one without is an owner, as
every superuser was before 2.0.

| Role | May |
|---|---|
| owner | everything |
| admin | everything but superusers and roles, restoring backups, deleting projects and the SQL console |
| developer | read everything but backups' files; write collections, records and files; run cron jobs |
| viewer | read everything but backups' files |

Every superuser may sign in, refresh their session and change their own password. A request
the role doesn't allow gets `403`. There's always at least one owner.

The role applies however a request is made: a collection's id means the same as its name, and
each request inside a batch or an import is held to the role too (a batch that contains one the
role forbids fails, and nothing in it is committed). **API keys** act for their superuser with at
most their own role, and never change the credentials of that account or end its sessions: a
read-only key can't take over the account it belongs to. The **SQL console** is for owners only,
since it reads every table, password hashes and the signing secret included.

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

**Record bodies** set fields by name. `tags+` / `+tags` append / prepend to a multi-valued field
and `tags-` removes from it; `views+` / `views-` add to / subtract from a number. A text field's
`autogeneratePattern` (any regular expression, e.g. `INV-[0-9]{6}`) fills it on create when
empty, and `code:autogenerate` sets it to the given value followed by a generated one.

**Auth collections** add the `password`, `tokenKey`, `email`, `emailVisibility` and `verified`
system fields, and auth options: `authRule`, `manageRule`, `passwordAuth`, `oauth2`,
`otp`, `mfa`, `authAlert`, token durations, and email templates. Users sign up by creating a
record with `password` and `passwordConfirm`, or through OAuth2.

**View collections** are read-only records from a SQL query, e.g.
`SELECT author AS id, count(*) AS posts FROM posts GROUP BY author`. Collections are tables named
after their collection. A view's fields come from the query's columns, and list/view rules,
filters, sorting and expand work as usual. A column taken from a collection is a copy of that
field (a select keeps its values, a relation its collection, a file field its options), found
through PostgreSQL's own record of where each column comes from, so joins, subqueries and CTEs
work too; a collection's `id` under another name is a relation to it. Other columns get a field
for their type (`count(*)` is a whole number). Options you set on a view's fields (presentable,
hidden, help) are kept when its query changes; a hidden source field stays hidden, and an auth
collection's password hash is hidden text. A view record's files are served from the record
they came from, after the view's own rule is checked. The query runs with the database role's privileges,
and a `SELECT` can call any function the role may use, so creating or changing a view requires
the SQL console to be enabled (`--sql-console`). Existing views keep working without it.

Before saving a view, `POST /api/collections/meta/dry-run-view` with `{"query": "SELECT ..."}`
returns the fields slonbase would make of it and its first rows, `{"fields": [...], "sample":
[...]}`, which is what the dashboard's view editor shows as you type. The query runs read-only
(it can't change data, even through a function, unless the database has an extension that
connects elsewhere, like `dblink`) on a connection that's closed afterwards, with a
15-second limit, and a query that can't
make a view (PostgreSQL refuses it, or its rows have empty or repeated ids) gets a 400 with
the reason.

**System collections:** superusers are records of the `_superusers` auth collection, and linked
OAuth2 accounts are records of `_externalAuths`, so both are managed with the
records API. System collections can't be renamed or deleted, and the last superuser can't be
deleted.

**Indexes** are given as `CREATE [UNIQUE] INDEX` statements, including partial
indexes with a `WHERE` clause.

## Sign-in

- **Password:** `auth-with-password`, with the identity fields set in `passwordAuth`.
- **OAuth2:** Google, Apple, GitHub, GitLab, Discord, Microsoft, Facebook, Bitbucket, Twitch,
  Spotify, Patreon, Kakao, X (`twitter`), Box, Gitea/Forgejo (`gitea`), Gitee, Instagram
  (`instagram2`), Lark, Linear, LiveChat, mailcow (with your URLs), monday.com (`monday`),
  Notion, Planning Center (`planningcenter`), Strava, Trakt, WakaTime, Yandex, and any OpenID
  Connect provider (`oidc`, `oidc2`, `oidc3` with your URLs), with PKCE (except Linear). An OpenID Connect provider
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

**Fields** picks what a JSON answer holds, as in PocketBase: any successful one (records,
auth, settings, collections, batch, routes), and realtime records.

- `fields=id,title` keeps those keys; in lists, of each item (the page envelope stays).
- Paths go into objects and lists of objects: `expand.author.name`, `meta.tags.label`.
- `*` keeps every key at its level: `*,expand.author.id`.
- `:excerpt(max, withEllipsis?)` turns HTML into a plain-text excerpt of about `max`
  characters, e.g. `fields=id,title,body:excerpt(200,true)`; other values are left as they are.
- A `fields` that can't be parsed (`body:excerpt(0)`, unbalanced parentheses) gets a 400.

## Compression

JSON answers over 1 KB are gzipped when the client accepts it (`Accept-Encoding`; browsers
and the SDKs do), so a page of 30 records is about a sixth of its size on the wire. Answers
that carry a token or a secret are never compressed, so an attacker who can inject text into a
page can't learn them from the compressed length (BREACH): sign-ins, token refreshes,
impersonation, file tokens, setup, Apple client secrets, and any JSON answer with a `"token"`
key, so a custom route that signs users in is covered too. Neither are files, which are served
as stored, nor answers marked `Cache-Control: no-transform`.

Compressing a page of records takes about 25 µs of CPU. Where bandwidth costs nothing (the
clients on the same network, say), `--no-compression` (or `compression = false` in the
`[server]` config) turns it off.

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

# Migrating from PocketBase

slonbase implements PocketBase's API (0.23 and later), so most apps move over by pointing the SDK
at the new URL. `slonbase import-pocketbase` brings over the data.

## Importing the data

```sh
slonbase setup --database-url 'postgres://...' --email you@example.com   # once
slonbase import-pocketbase /path/to/pb_data      # or a PocketBase backup .zip
slonbase serve
```

The importer reads PocketBase 0.23+ data (`data.db` and `storage/`) without changing it, and
brings over:
- collections with their ids, fields, rules, indexes and auth options (OAuth2 providers,
  templates, MFA/OTP settings);
- records with their ids and timestamps, including password hashes, so users sign in with their
  passwords as before (both use bcrypt);
- superusers (except emails slonbase already has), OAuth2 account links, files (thumbnails are
  made again on request) and settings (SMTP, S3, backups, rate limits, ...).

Stop PocketBase first, and import into an installation that doesn't have those collections yet.
Things it can't carry over are reported instead of failing the import:
- view collections whose queries use SQLite-only SQL (`strftime`, ...): recreate them with a
  PostgreSQL query;
- settings encrypted with `PB_ENCRYPTION`: set them again in the dashboard;
- files kept in S3 by PocketBase: copy them to slonbase's storage under the same
  `{collectionId}/{recordId}/` keys.

Records are written directly: hooks and realtime events don't run for them. The import is
tested end to end in CI against the PocketBase release (`compat/import/run.sh`).


The command prints a report: what was imported, and each thing it couldn't carry over, with
the reason.

## Pointing your app at slonbase

```js
const pb = new PocketBase("https://new.example.com");   // the only change for most apps
```

The official JavaScript SDK is tested against slonbase in CI, and the Dart SDK uses the same
API. Tokens issued by PocketBase aren't valid
on slonbase, so users sign in again. Their passwords keep working.

## JavaScript hooks

Copy `pb_hooks/*.pb.js` to slonbase's hooks directory. The [extending guide](extending.md) lists
everything that's available. The differences to watch for:

- **SQL is PostgreSQL.** Queries through `$app.db().newQuery(...)` and in view collections run
  on PostgreSQL, so SQLite-specific SQL (`strftime`, `json_extract`, `datetime('now')`, ...)
  needs porting. Collections are tables in the `slonbase` schema, named after the collection.
- **Not available yet:** the collection request hooks (`onCollectionsListRequest`, ...),
  `onBatchRequest`, the `*Execute` and `*AfterError` model hooks, and the `$dbx` query builder.
  Registering a missing hook fails with a `ReferenceError` when the file loads, so you'll know
  right away.
- **`onRealtimeConnectRequest`'s `e.next()` returns once the connection is open**, where
  PocketBase's returns when it closes. Code after it runs at connect time.
- **Realtime connections have no idle or maximum timeout** unless a connect handler sets
  `e.idleTimeout` or `e.maxTimeout` (PocketBase closes them after 5 minutes idle and 30 minutes
  in all; the SDKs reconnect either way).
- **`$template` escapes everything the same way**, where Go escapes by context (inside
  `<script>`, URLs, ...). Use `js` and `urlquery` there.
- **The engine is QuickJS** rather than Goja. It's standard JavaScript (ES2023), with no Go types.
- Go extensions (`main.go`) can't be carried over. slonbase's equivalent is its
  [Rust library](extending.md#rust).

## Migrations

Copy `pb_migrations/*.js` to slonbase's migrations directory: PocketBase's migration files,
automigrate output and snapshots run as they are (see [migrations](migrations.md)). The import
marks PocketBase's applied migrations as applied, so they don't run again.

## Other differences

| | PocketBase | slonbase |
|---|---|---|
| Database | Embedded SQLite | Your PostgreSQL 14+ server (not bundled) |
| Instances | One | Any number on one database (realtime needs [client affinity](deployment.md#several-instances)) |
| Your data | SQLite file | PostgreSQL tables you can query, replicate and back up with the usual tools |
| Write durability | `synchronous = NORMAL` | `synchronous_commit = on` by default ([details](deployment.md#postgresql)) |
| Backups | Zip with the SQLite file | Zip in slonbase's own format, plus any PostgreSQL backup tool |
| Settings encryption | `PB_ENCRYPTION` | Not supported; protect the database instead |
| Migrations | Go/JS migrations, automigrate | [JS migrations](migrations.md) and automigrate; Go migrations aren't supported |
| Rate limits | Per process, off on new installations | Per instance, on for new installations (an import keeps PocketBase's setting) |
| Dashboard | PocketBase's | slonbase's own, with the same features |

Error messages and validation codes follow PocketBase (e.g. `validation_required`,
`validation_min_text_constraint`), so form errors in your UI keep working.

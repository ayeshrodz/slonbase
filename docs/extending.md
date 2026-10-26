# Extending slonbase

slonbase can be extended in two ways. [JavaScript hooks](#javascript-hooks) are `*.pb.js`
files that use PocketBase's JavaScript API and need no build step. The [Rust library](#rust) lets
you build your own binary with native hooks and routes.

## JavaScript hooks

Put `*.pb.js` files in the hooks directory: `slonbase_hooks` by default, `/data/hooks` in Docker,
or `/var/lib/slonbase/hooks` for the packages. slonbase loads them on start and restarts when
they change. They use PocketBase's JavaScript API, so most PocketBase hooks work as they are
(see [migrating](migrating-from-pocketbase.md#javascript-hooks) for what's different). The engine
is [QuickJS](https://bellard.org/quickjs/) (ES2023); there's no Node.js API or npm.

```js
// slonbase_hooks/main.pb.js
onRecordCreateRequest((e) => {
  e.record.set("status", "draft");
  e.next(); // validates and saves the record
}, "posts");

onRecordAfterCreateSuccess((e) => {
  console.log("new post", e.record.id);
  e.next();
}, "posts");

routerAdd("GET", "/api/hello/{name}", (e) => {
  const posts = $app.findRecordsByFilter("posts", "status = {:s}", "-created", 10, 0, { s: "draft" });
  return e.json(200, { hello: e.request.pathValue("name"), drafts: posts.length });
}, $apis.requireAuth());

cronAdd("cleanup", "0 3 * * *", () => {
  $app.db().newQuery("DELETE FROM posts WHERE status = {:s}").bind({ s: "spam" }).execute();
});
```

### What's available

**Hooks:**

| Hook | When it runs |
|---|---|
| `onRecord{Create,Update,Delete,View}Request`, `onRecordsListRequest` | Around record API requests. A handler can change `e.record`, throw, or answer itself instead of calling `e.next()`. |
| `onRecord{Create,Update,Delete}` | Model hooks: every write (API, batches, `$app.save`, the Rust record service), inside its transaction, before the record is written |
| `onRecordValidate` | Every create and update, after the built-in validation passed |
| `onRecordAfter{Create,Update,Delete}Success` | After the change is committed, in the background |
| `onCollection{Create,Update,Delete}`, `onCollectionAfter{Create,Update,Delete}Success` | Changes through the collections API, imports included |
| `onRecordAuthRequest` | Every sign-in, token refresh and impersonation, before the MFA check. A handler can change `e.record` (in the response only), `e.token` and `e.meta` (`e.authMethod` says how the record signed in), or throw. |
| `onRecordAuthWithPasswordRequest`, `onRecordAuthWithOAuth2Request`, `onRecordAuthWithOTPRequest`, `onRecordAuthRefreshRequest` | Around each sign-in. `e.record` is null when no record matched the identity, and a handler can set it, to find records its own way. Password: `e.identity`, `e.identityField`, `e.password`; OAuth2: `e.providerName`, `e.oauth2User`, `e.createData`, `e.isNewRecord`; OTP: `e.otp`. |
| `onRecordRequestOTPRequest`, `onRecordRequest{Verification,PasswordReset,EmailChange}Request`, `onRecordConfirm{Verification,PasswordReset,EmailChange}Request` | Around the email flows. `e.password` is the one-time password about to be sent, `e.newEmail` the new address. A handler that doesn't call `e.next()` skips the action (and the email). |
| `onRecordEnrich` | Every record sent to a client: in responses, batches, auth responses, expanded relations and realtime messages. `e.record.set()` adds keys, `hide()` and `unhide()` change which fields are sent (superusers always get every field). |
| `onMailerSend`, `onMailerRecord{Verification,PasswordReset,EmailChange,OTP,AuthAlert}Send` | Before an email is sent |
| `onRealtimeConnectRequest` | A realtime connection being opened: `e.client` (`id()`, `subscriptions()`, `get("auth")`), `e.idleTimeout`, `e.maxTimeout` (nanoseconds, like Go's durations; 0, the default, means none). Throw to refuse it. Unlike PocketBase, `e.next()` returns once the connection is open, not when it closes. |
| `onRealtimeSubscribeRequest` | A client setting its subscriptions: change `e.subscriptions` (topic strings) or throw |
| `onRealtimeMessageSend` | Every message to a realtime client, `PB_CONNECT` included: `e.message.name`, `e.message.data` (JSON text; `toString()` works as in PocketBase). Skip it by not calling `e.next()`; throwing closes the connection. |
| `onFileDownloadRequest` | A file download (can be limited to collections): `e.record`, `e.fileField`, `e.servedPath` (a storage key), `e.servedName`, `e.thumbError`. Change what's served, throw, or answer yourself. |
| `onFileTokenRequest` | A protected-file token request (can be limited to the caller's collection): `e.record` (the caller), `e.token` |
| `onSettingsListRequest`, `onSettingsUpdateRequest` | The settings API: `e.settings` (secrets masked); `e.oldSettings` and `e.newSettings` (what's saved, validated like the request) |
| `onSettingsReload` | After the settings changed (here, on another instance, after a restore) and once at startup, in the background: `e.settings` |
| `onBackupCreate`, `onBackupRestore` | Every backup (the API, automatic backups, `$app.createBackup`) and restore: `e.name` can be changed; `e.exclude` is accepted but ignored (backups always hold the database and the files). Throw to cancel. |
| `onTerminate` | When the server stops, after it stopped taking requests: `e.isRestart` (a restart after the hook files changed). Handlers get at most 5 seconds. |
| `onBootstrap` | Once the app is ready |
| `routerAdd(method, path, handler, ...middlewares)` | A custom route. Middlewares: `$apis.requireAuth(...collections)`, `requireSuperuserAuth()`, `requireSuperuserOrOwnerAuth(param)`, `requireGuestOnly()`. |
| `routerUse(handler)` | Middleware around every request; `e.next()` continues |
| `cronAdd(id, expression, fn)`, `cronRemove(id)` | Scheduled jobs (UTC). They run on one instance per minute and show in the dashboard. |

Record and file hooks take collection names (or ids) after the handler to limit them to
those collections.

**Globals:**

- `$app`: `findRecordById`, `findRecordsByFilter`, `findFirstRecordByFilter`,
  `findAuthRecordByEmail`, `countRecords`, `findCollectionByNameOrId`, `findAllCollections`,
  `save` and `delete` (of records, collections and, for `save`, the settings),
  `importCollections`, `db().newQuery(sql).bind({...}).execute()` / `.one()` / `.all()`
  (PostgreSQL SQL, with `{:name}` placeholders), `settings()`, `newMailClient().send(...)`,
  `createBackup(name)` and `restoreBackup(name)` (PocketBase's context argument is accepted
  and ignored).
- `Collection`, `Field` (and `TextField`, `NumberField`, ...), `unmarshal`: collection
  definitions, as in [migrations](migrations.md#whats-available).
- `Record`: `new Record(collection)`, `get`/`set`/`getString`/`getInt`/`getBool`/...,
  `email()`, `setPassword`, `validatePassword`, `hide`/`unhide`, and tokens:
  `newAuthToken`, `newStaticAuthToken(duration)`, `newVerificationToken`,
  `newPasswordResetToken`, `newEmailChangeToken(newEmail)`, `newFileToken`.
- `$mails`: `sendRecordVerification($app, record)`, `sendRecordPasswordReset`,
  `sendRecordChangeEmail(app, record, newEmail)`, `sendRecordOTP(app, record, otpId, password)`,
  `sendRecordAuthAlert(app, record, info)`: the emails the auth endpoints send, with the
  collection's templates.
- `$template`: Go templates, rendered to HTML (see [below](#templates)).
- `$filesystem`: `fileFromBytes`, `fileFromPath`, `fileFromURL` (`http(s)` only); in routes,
  `e.findUploadedFiles(key)` returns a multipart request's files. Files are limited to 32 MiB.
- `$http.send({url, method, headers, body, timeout})`.
- `$security`: `randomString`, `randomStringWithAlphabet`, `sha256`, `sha512`, `hs256`,
  `md5`, `equal`, ...
- `$os.getenv`, `console.log/info/warn/error`, and `require()` for modules in the hooks
  directory.
- Errors: `BadRequestError`, `NotFoundError`, `ForbiddenError`, `UnauthorizedError` and
  `ApiError(status, message, data)` become API errors with that status. Other errors answer 400
  and are logged.

### Templates

`$template` renders Go's `html/template` language, like PocketBase:

```js
routerAdd("GET", "/hello/{name}", (e) => {
  const html = $template
    .loadFiles(`${__hooks}/views/layout.html`, `${__hooks}/views/hello.html`)
    .render({ name: e.request.pathValue("name") });
  return e.html(200, html);
});
```

The first file is rendered, and it usually uses blocks the others `define`
(`{{template "content" .}}`). `loadString(text)` works the same for a single template, and
`$template.addFuncs({ upper: (s) => s.toUpperCase() })` adds functions templates can call.
Everything printed is HTML-escaped, except what `raw` returns. Unlike Go, the escaping doesn't
depend on where the value is: inside `<script>` or a URL, use the `js` and `urlquery`
functions. The compatibility suite checks templates against PocketBase's output.

### How it runs

- **Runtimes:** each of the `--hooks-pool` runtimes (10 by default) loads every file, so
  top-level code runs once per runtime. Keep state in the database rather than in variables.
  When all of them are busy, extra runtimes start, and they stop again when idle. Handlers can
  nest: a `routerUse` handler waits while the route runs, and the route may run a model hook.
- **Model hooks** run before the record is written, inside its transaction. Changes to
  `e.record` are written, and throwing cancels the write. The record is written once the
  handlers return, so code after `e.next()` still sees it unsaved; use
  `onRecordAfter*Success` for work after the write. Don't write the same record through `$app`
  from its own model hook; change `e.record` instead. Records deleted by a cascade run
  `onRecordDelete` too, and a handler's error cancels the whole delete.
- **Validation hooks** run after the built-in validation passed. Throw (e.g. a
  `BadRequestError` with field data) to reject the record.
- **Collection hooks** see `e.collection`, a `Collection` with the collection's JSON. Create and update handlers
  may change it before it's validated and applied.
- **Mail hooks** see `e.message` (`to`, `subject`, `html`) and, for auth emails, `e.record` and
  `e.kind`. A handler that doesn't call `e.next()` skips the email.
- **Files:** uploaded and `$filesystem` files can be set on a file field before
  `$app.save(record)`. When they're mixed with stored file names, the names are kept and the
  files are appended.
- **Reloading:** when the hook files change, slonbase restarts itself. It's the same process,
  re-executed with the same arguments, after up to 5 seconds for open requests. Turn this off
  with `hooks.watch = false`.

Build with `--no-default-features` to leave the JavaScript engine out.

## Rust

slonbase is also a Rust library. To add hooks, routes and cron jobs in Rust, make a binary
crate that depends on slonbase and runs its command line from your own `main`:

```toml
[dependencies]
slonbase = { git = "https://github.com/ayeshrodz/slonbase", tag = "v0.4.2" }
axum = "0.8"
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
```

```rust
use axum::routing::get;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let mut ext = slonbase::Extensions::default();
    // runs before the record is written; `next` writes it
    ext.hooks.on_record_create_request.add(&["posts"], |mut e, next| async move {
        e.record.insert("status".into(), "draft".into());
        next.run(e).await
    });
    // runs after the change is committed (API writes, batches and the record service)
    ext.hooks.on_record_after_create_success.add(&["posts"], |e, next| async move {
        tracing::info!(id = %e.record["id"], "new post");
        next.run(e).await
    });
    ext.hooks.cron("nightly", "0 3 * * *", |core| async move {
        let old = core.count_records("posts", "created < {:t}", &serde_json::Map::from_iter([
            ("t".into(), "2025-01-01".into()),
        ])).await?;
        tracing::info!(old, "old posts");
        Ok(())
    })
    .expect("a valid cron expression");
    ext.route("/api/hello", get(|| async { "Hello!" }));
    slonbase::cli::run(ext).await
}
```

Hooks: `on_record_{create,update,delete,view}_request`, `on_records_list_request` (around API
requests; a hook can change the record, return an error, or set `e.response` instead of calling
`next`) and `on_record_after_{create,update,delete}_success` (after commit, in the background).
`hooks.on_record_create`/`_update`/`_delete`, `on_record_validate`, `on_collection_*` and
`on_mailer_send` are the model, validation, collection and mail hooks described above.
`hooks.on_request` is middleware around every request (`e.request`, `e.auth`; `next` routes it and
sets `e.response`). `hooks.on_ready` runs when the app is ready. Cron jobs run on one instance per scheduled minute
and show up in the dashboard. Hooks, routes and cron jobs can read and write records with
`core.find_record_by_id`, `find_records_by_filter`, `find_first_record_by_filter`,
`count_records`, `save_record` and `delete_record` (no API rules; filters take `{:name}`
placeholders).


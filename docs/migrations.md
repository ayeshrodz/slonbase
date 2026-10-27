# Migrations

Migrations keep your collections (and any data or settings they need) in code, so every
installation gets the same schema: your laptop, staging, production, and each new server. They're
JavaScript files, applied in order, each in a transaction. (Migrations written for PocketBase
run too; see [below](#coming-from-pocketbase).)

> [!NOTE]
> Migrations need the JavaScript engine (the default build).

- [Writing a migration](#writing-a-migration)
- [When they run](#when-they-run)
- [Automigrate](#automigrate)
- [The command line](#the-command-line)
- [What's available](#whats-available)
- [Coming from PocketBase](#coming-from-pocketbase)

## Writing a migration

A migration is a `.js` file in the migrations directory (`slonbase_migrations`, see
[configuration](configuration.md)). It calls `migrate()` with an `up` function, which makes
the change, and optionally a `down` function, which undoes it:

```js
// slonbase_migrations/1730000000_create_posts.js
migrate((app) => {
  const posts = new Collection({
    name: "posts",
    listRule: "",
    viewRule: "",
    fields: [
      { name: "title", type: "text", required: true, max: 200 },
      { name: "published", type: "bool" },
    ],
  });
  app.save(posts);
}, (app) => {
  app.delete(app.findCollectionByNameOrId("posts"));
});
```

`slonbase migrate create create_posts` writes an empty one with the right name. Files run in
the order of their names, which start with a Unix timestamp, so new ones go last.

**Each migration runs in a transaction of its own.** Everything it does applies together, or,
if it throws, not at all, and the migrations after it wait. That covers collection changes,
records and raw SQL alike. A call that fails inside a `try` doesn't spoil the transaction: the
migration can catch the error and go on.

## When they run

The server applies pending migrations when it starts, before it answers requests. Each file
runs once: the applied ones are kept in the database (`_slonbase.app_migrations`), so they're
applied once per database, not once per server.

- **Several instances** starting at once apply each migration once.
- **A failing migration** keeps the server starting. `/api/health` answers `503`, and the error
  is its status and in the log. slonbase tries again with a growing delay (up to a minute), so
  fixing the file is enough: no restart needed.
- **After the setup wizard**, pending migrations run before the dashboard opens.
- **Backups** include the list of applied migrations, so a restored backup and its migrations
  stay in step.
- **Projects** each have their own migrations, in `<projects.path>/<name>/migrations`.

## Automigrate

With automigrate on (`--automigrate`, `SLONBASE_AUTOMIGRATE=true` or
`migrations.automigrate = true`), every collection change you make in the dashboard or through
the collections API is also written as a migration file:

```
slonbase_migrations/1730000123_created_posts.js
slonbase_migrations/1730000187_updated_posts.js
slonbase_migrations/1730000201_deleted_drafts.js
```

The files are marked as applied, since the changes already are. Commit them, and the other
installations get the changes when they start. Each file holds the whole collection definition
before and after, so `down` restores the previous version.

Automigrate is off by default. Turn it on where you design the schema (usually your own
machine), and keep it off in production, where the changes come from the files.

## The command line

```sh
slonbase migrate list               # every file, applied or pending
slonbase migrate up                 # apply the pending ones (the server also does)
slonbase migrate down               # revert the last one; `down 3` reverts the last three
slonbase migrate create add_tags    # a new, empty migration
slonbase migrate collections        # a snapshot: one migration that recreates every collection
slonbase migrate history-sync       # forget applied migrations whose files you deleted
```

Use the same `--config` (or environment) as the server, and `--project <name>` for a project.
For the packages, run them as the service user:
`sudo -u slonbase slonbase --config /etc/slonbase/config.toml --migrations-dir /var/lib/slonbase/migrations migrate list`.

`migrate collections` is handy to start using migrations on an existing installation: the
snapshot describes the collections as they are, so it's marked as applied here and creates
them on a new database.

## What's available

Migrations get `app`, the same API as `$app` in [hooks](extending.md#whats-available), with
every call running in the migration's transaction.

**Collections**

| | |
|---|---|
| `new Collection({...})` | A collection definition, in the collections API's JSON (`name`, `type`, `fields`, rules, `indexes`, auth options, `viewQuery`) |
| `app.save(collection)` | Creates it, or updates the collection with its id (or name). Validated like the API. |
| `app.delete(collection)` | Deletes it with its records |
| `app.findCollectionByNameOrId(name)`, `app.findAllCollections(...types)` | |
| `app.importCollections(list, deleteMissing)` | Creates or updates several at once; with `deleteMissing`, deletes the other non-system collections |
| `collection.fields` | A list of fields with `add(...fields)`, `addAt(pos, ...fields)`, `getById`, `getByName`, `removeById`, `removeByName`, `fieldNames()` |
| `new Field({...})`, `new TextField({...})`, `NumberField`, `BoolField`, `EmailField`, `URLField`, `EditorField`, `DateField`, `AutodateField`, `SelectField`, `FileField`, `RelationField`, `JSONField`, `GeoPointField`, `PasswordField` | Fields, in the API's JSON |
| `unmarshal(data, collection)` | Copies JSON keys onto a collection (or a record) |

**Records and data**: `new Record(collection)`, `app.save(record)`, `app.delete(record)`,
`app.findRecordById`, `app.findRecordsByFilter`, `app.findFirstRecordByData`,
`app.findAuthRecordByEmail`, `app.countRecords`, `app.recordQuery(collection)`, and
`app.db()` with `$dbx` for SQL (PostgreSQL; see [SQL](extending.md#sql)). Collections are
tables in the `slonbase` schema, and unqualified names find them.

**Settings**: `const settings = app.settings(); settings.meta.appName = "Acme"; app.save(settings);`

For example, a first superuser and some data:

```js
migrate((app) => {
  const superuser = new Record(app.findCollectionByNameOrId("_superusers"));
  superuser.set("email", "admin@example.com");
  superuser.set("password", $os.getenv("ADMIN_PASSWORD"));
  app.save(superuser);

  app.db().newQuery("UPDATE posts SET published = true WHERE title <> ''").execute();
});
```

Notes:
- View collections can be created from migrations without the SQL console: migrations are
  code you deploy, like hooks.
- Collection hooks don't run for changes made by migrations. Record hooks do when the server
  applies them, but not with `slonbase migrate up`, which doesn't load the hooks.
- `e.next()`, routes and cron jobs aren't available in migrations.

## Coming from PocketBase

Copy `pb_migrations/*.js` to the migrations directory. The files PocketBase writes (its
automigrate output and `pocketbase migrate collections` snapshots) are tested against slonbase
in CI.

- **`slonbase import-pocketbase` brings the history along:** PocketBase's applied migrations
  are marked as applied, so they don't run again on the imported collections.
- **PocketBase's system collections** in snapshots and imports: `_superusers` and
  `_externalAuths` take the settings (rules, auth options) but keep slonbase's fields, and
  `_mfas`, `_otps` and `_authOrigins` (tables of slonbase's own) are skipped.
- **SQL is PostgreSQL** in `app.db()` queries and view queries.
- Migrations written for PocketBase before 0.23 (`migrate((db) => { const dao = new Dao(db) ...`)
  use an API that PocketBase itself replaced; rewrite them with `app`.

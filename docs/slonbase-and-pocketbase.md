# slonbase and PocketBase: how they differ

PocketBase is an open-source backend in a single Go binary, with SQLite inside. It inspired
slonbase, and slonbase keeps its API shape so that PocketBase's client SDKs, hooks and data carry
over. Beyond that they are different programs: slonbase is a separate codebase in Rust, built for
PostgreSQL, with its own dashboard and a wider set of features. This page says what is shared,
what differs, and which to pick.

## What is shared

- **The API's shape:** routes, filter and sort syntax, response and error formats, realtime
  protocol. The PocketBase JavaScript and Dart SDKs work against slonbase.
- **JavaScript hooks and migrations:** most `pb_hooks` and `pb_migrations` files run unchanged.
- **The data model:** collections of base, auth and view types, with the same field types and rules.
- **A migration path:** `slonbase import-pocketbase` brings over collections, records, files,
  settings and password hashes. See [migrating](migrating-from-pocketbase.md).

A test suite written with the official SDK runs against both servers in CI, and a differential
runner sends both the same requests and compares the answers.

## What differs

| | slonbase | PocketBase |
|---|---|---|
| Database | PostgreSQL 14+ (yours, or managed) | SQLite, inside the process |
| Your data | Real tables: SQL, replicas, `pg_dump`, reporting tools | A SQLite file |
| Instances | Any number on one database, behind any load balancer | One |
| Projects | Many isolated projects in one installation, with a command center | One installation per process |
| Realtime | Every change to a table, including SQL and other programs | Changes made through the app |
| Dashboard | Its own: SQL console, designers for emails and the sign-in page, branding, command center | Its own, simpler |
| Team | Roles (Owner, Admin, Developer, Viewer) and API keys | Superusers |
| Operations | Job history, encrypted backups, lockout, rate-limit tester, metrics | Backups, logs |
| Extending | JavaScript hooks, or Rust | JavaScript hooks, or Go |
| Moving parts | One binary and a PostgreSQL server | One binary |

## Which to choose

**Choose slonbase** when you want a backend on PostgreSQL: SQL access to your data, several
instances, managed databases and replicas, many projects in one place, or the speed of
PostgreSQL's planner on lists and filters (see the [benchmarks](benchmarks.md)).

**Choose PocketBase** when you want zero moving parts: one file with SQLite inside, nothing else to
run, one small app on one small machine, or you extend it in Go.

slonbase is an independent project and isn't affiliated with PocketBase, a project by Gani
Georgiev.

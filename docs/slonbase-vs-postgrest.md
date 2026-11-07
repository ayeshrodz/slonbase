# slonbase vs PostgREST

PostgREST turns a PostgreSQL schema into a REST API. slonbase is a whole backend platform on
PostgreSQL that includes one. They solve different amounts of the problem, so the question is how
much you want built in.

## What PostgREST is

A single, very lean program that serves your tables, views and functions as a REST API and checks
JWTs, leaving authorization to PostgreSQL's row-level security. You design and migrate the schema
yourself. It has no sign-in or user management, no file storage, no realtime and no dashboard.

## What slonbase adds

| | slonbase | PostgREST |
|---|---|---|
| REST API over PostgreSQL tables | Yes: collections are tables | Yes |
| Sign-in and users | Passwords, OAuth2, one-time codes, MFA, email flows | You issue the JWTs |
| Access control | Per-collection rules in one filter language | PostgreSQL row-level security |
| Files | Disk or S3, thumbnails, protected files | No |
| Realtime | Every change, per-subscriber rules | No |
| Dashboard | Records, schema, rules, logs, SQL console, many projects | No |
| Hooks and extending | JavaScript hooks, cron jobs, Rust library | PostgreSQL functions |
| Schema changes | From the dashboard or migrations | Your SQL migrations |
| Moving parts | One binary and PostgreSQL | One binary and PostgreSQL |

## Speed

Measured on the same machine, the same data and the same HTTP workloads, with PostgREST serving
the tables slonbase created ([benchmarks](benchmarks.md)): slonbase answered about 2.1 times as
many list requests per second (7,814 against 3,733), 3.8 times as many single-record reads and 1.9
times as many creates. PostgREST is much lighter in memory (a 15 MB process against about 40 MB)
and smaller to download, since it does much less. PostgREST was run at its defaults with a pool
of 16; a tuned PostgREST may do better.

## Which to choose

**Choose PostgREST** when you already manage your PostgreSQL schema and only need a fast REST layer
in front of it, with row-level security, and you handle sign-in, files and realtime elsewhere.

**Choose slonbase** when you want the rest of the backend too: users and sign-in, rules, files,
realtime, hooks and an admin dashboard, in one program.

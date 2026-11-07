# About slonbase

slonbase is an open-source backend platform on PostgreSQL. One Rust binary gives you a REST and
realtime API, authentication with roles, file storage, JavaScript hooks and an admin dashboard
that manages many projects, with every collection kept as a real PostgreSQL table. It is
self-hosted, runs as several instances behind a load balancer, and works with the PocketBase SDKs.

## What it is for

You define your data in a dashboard (or in SQL) and get the backend of an app without writing
one: sign-in, access rules, an API, realtime updates, files, emails and an admin interface. The
data stays in a PostgreSQL database you own, so you can query it with SQL, back it up with
`pg_dump`, replicate it and connect your reporting tools to it.

It suits teams and developers who:

- want a backend they host themselves, on their own PostgreSQL (or a managed one);
- build several apps or client projects and want one place to run and manage them;
- want one small program to deploy instead of a set of services;
- already use, or want to use, PocketBase's client SDKs and hooks.

## What it is not

- **Not a database.** It stores your data in PostgreSQL 14 or newer, which it doesn't bundle.
- **Not a hosted service.** You run it; there is no account to create. (Managed PostgreSQL
  services work as the database.)
- **Not a clone or a fork of PocketBase.** See [slonbase and PocketBase](slonbase-and-pocketbase.md).
- **Not Solbase,** the unrelated tool for deploying tokens on the Solana and Base networks.

## Where it came from

The idea began with PocketBase, which showed how good a whole backend in one file can feel, and
with wanting that on PostgreSQL. slonbase is a separate codebase, written from scratch in Rust
with its own design, dashboard and features. It keeps PocketBase's API shape on purpose, so
existing SDK code, hooks and data carry over ([migrating](migrating-from-pocketbase.md)), and it
has grown well beyond that: projects, roles, an SQL console, designers for emails and the
sign-in page, realtime for every change and scale-out across instances. The name is *slon*
(elephant, for PostgreSQL's mascot) and *base*.

## Who builds it

slonbase is built and maintained by [Ayesh Rodrigo](https://ayesh.dev/), a software engineer, as
an independent open-source project under the MIT license. The source, issues and releases are on
[GitHub](https://github.com/ayeshrodz/slonbase). PocketBase is a separate project by Gani
Georgiev; slonbase isn't affiliated with it.

## Facts

| | |
|---|---|
| License | MIT |
| Written in | Rust (the dashboard in plain JavaScript, no build step) |
| Database | PostgreSQL 14 or newer |
| Platforms | Linux on x86_64 and aarch64 (static binaries, `.deb` and `.rpm` packages, a Docker image); other platforms from source |
| Install | `curl -fsSL https://slonbase.com/install.sh \| sudo sh`, or Docker, or a binary |
| Stability | Semantic Versioning; see [stability](stability.md) |
| Website | <https://slonbase.com> |

Next: the [features](features.md), the [quickstart](quickstart.md) and the
[benchmarks](benchmarks.md).

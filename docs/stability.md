# Stability

From 1.0, slonbase follows [Semantic Versioning](https://semver.org/) for everything this page
lists: within 1.x, an upgrade doesn't break what works. A breaking change waits for 2.0, and
the changelog lists it with what to do.

Until then (0.x), a minor version may break things, and the changelog's **Upgrading** section
says what. This page is what 1.0 promises.

## What's covered

**The HTTP API.** The endpoints in the [API guide](api.md), their parameters, the shape of their
responses, and the error format (`status`, `message`, `data`). The filter and sort syntax, the
API rule syntax, field types and their JSON values, tokens, and the realtime protocol. What the
PocketBase JavaScript and Dart SDKs use keeps working with them.

**Configuration.** Command-line flags, `SLONBASE_*` environment variables, the config file's
keys (see [configuration](configuration.md)), and the application settings (`GET/PATCH
/api/settings`): their names, their meaning and their defaults. A new setting starts with a
default that keeps the old behavior.

**Your data.** Collections are PostgreSQL tables you own; slonbase changes them only when you
change the collection. The `_slonbase` schema is migrated forward on start, by any later 1.x
release. Files keep their storage layout, locally and in S3. A backup made by a 1.x release
restores on any later 1.x release, encrypted ones included.

**JavaScript hooks and migrations.** The globals and hooks in [extending](extending.md#javascript-hooks)
and [migrations](migrations.md), with their arguments and return values. Hooks and migrations
written for 1.0 run on every 1.x release.

**The Rust library.** The public API of the `slonbase` crate (see [extending](extending.md#rust)).
The minimum supported Rust version can rise in a minor release, announced in the changelog.

**Metrics.** The names and labels of the Prometheus metrics. New metrics and labels can appear.

## What isn't

- **The dashboard.** It changes freely: pages move, controls change, things get redesigned. It
  only uses the public API, so anything it does stays possible through the API.
- **The wording** of log messages, error messages and emails (their meaning stays; the text can
  improve). Match on status codes and fields, not on messages.
- **The `_slonbase` schema's tables.** Read them if you like, but they're slonbase's: they can
  change in any release, and writing to them isn't supported.
- **Anything undocumented**, and anything a guide marks as experimental.
- **Fixes.** When slonbase behaves differently from what the documentation says, the fix is not a
  breaking change, even if something relied on the bug. Security fixes in particular can
  tighten behavior in any release, explained in the changelog.

## What isn't a breaking change

New endpoints, new fields in responses, new optional parameters, new settings (with defaults
that keep the old behavior), new hooks and new globals. Clients should ignore fields they don't
know.

## Deprecations

Something to be removed is first deprecated in a minor release: the changelog says so and what
to use instead, the documentation marks it, and using it logs a warning. It keeps working
until the next major version.

## Upgrades

Upgrade from any 1.x release straight to the latest: stop the old version, start the new one,
and its migrations run on start (see [installation](installation.md)). CI upgrades a database
from the first release and from the previous one, with data, on every change.

Downgrades aren't supported: a newer release can migrate the database in ways an older one
doesn't understand. Take a backup before upgrading, and restore it to go back.

## Supported environments

- **PostgreSQL:** every version the PostgreSQL project still supports, from 14 (CI tests the
  oldest and the newest). Support for a version can end once the PostgreSQL project's ends,
  announced a release ahead.
- **Platforms:** Linux on x86-64 and ARM64, as packages, binaries and the Docker image.

## Security fixes

The latest minor release gets security fixes. From 1.0, the previous minor release gets them
too for three months after the next one comes out. See [the security policy](../SECURITY.md)
for reporting a vulnerability.

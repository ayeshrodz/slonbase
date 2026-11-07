# Benchmarks

slonbase, PocketBase and PostgREST on the same workloads, through their HTTP APIs: `seed.mjs`
creates the same data with the official SDK (100 authors, 10,000 posts with an index on `views`,
a user), and `run.sh` measures each scenario with [hey](https://github.com/rakyll/hey).
PostgREST has no collections or SDK, so it serves the tables slonbase created, in the same
PostgreSQL database (see [PostgREST](#postgrest) below for what that means for fairness).

## Results

slonbase **2.2.0** and 1.9.0 (the static musl release binaries as published), PostgreSQL 16 with
default settings, PocketBase **0.40.6** (the official release binary, default settings) and
PostgREST **16.5** (the official static binary, `db-pool` 16, otherwise default), on a 4-vCPU cloud VM (Intel Xeon @ 2.10GHz, 15 GB, Ubuntu 24.04): the
server (and PostgreSQL) pinned to 3 CPUs, the load generator to the fourth. Each scenario runs 3
times for 10 s after a 2 s warm-up; the tables show the medians, and ± is half the spread between
the runs. Compare within a table: the VM's speed varies from day to day (this one was a little
faster than the one for 1.9's numbers).

| Scenario | Clients | slonbase 2.2 req/s | PocketBase req/s | PostgREST req/s | slonbase / PocketBase | slonbase / PostgREST | p50 ms (slonbase / PB / PostgREST) | p99 ms (slonbase / PB / PostgREST) |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| health | 32 | 44,316 ±2% | 32,270 ±4% | 36,267 ±2% | **1.37×** | **1.22×** | 0.7 / 0.7 / 0.9 | 2.1 / 6.4 / 2.2 |
| view one record | 32 | 19,965 ±3% | 11,484 ±2% | 5,197 ±2% | **1.74×** | **3.84×** | 1.5 / 1.4 / 6.0 | 3.7 / 23.4 / 14.4 |
| view one record (1 client) | 1 | 5,765 ±3% | 3,188 ±1% | 1,726 ±3% | **1.81×** | **3.34×** | 0.2 / 0.2 / 0.5 | 0.4 / 1.0 / 1.2 |
| list 30 | 32 | 7,814 ±1% | 1,610 ±3% | 3,733 ±4% | **4.86×** | **2.09×** | 4.0 / 16.7 / 8.4 | 7.8 / 67.1 / 13.7 |
| list 30, sorted | 32 | 7,117 ±3% | 1,578 ±5% | 3,345 ±2% | **4.51×** | **2.13×** | 4.4 / 17.2 / 9.4 | 8.4 / 68.2 / 14.4 |
| list 30, filtered | 32 | 7,100 ±2% | 105 ±4% | 3,315 ±1% | **68×** | **2.14×** | 4.4 / 302 / 9.5 | 8.2 / 439 / 15.4 |
| list 30, expand author | 32 | 3,906 ±1% | 1,065 ±2% | n/a | **3.67×** | n/a | 8.1 / 24.7 / n/a | 12.6 / 93.3 / n/a |
| list 30, signed in | 32 | 6,321 ±2% | 1,534 ±4% | n/a | **4.12×** | n/a | 5.0 / 17.7 / n/a | 8.5 / 67.3 / n/a |
| create record | 32 | 7,424 ±3% | 4,051 ±2% | 3,828 ±1% | **1.83×** | **1.94×** | 4.1 / 4.6 / 8.1 | 8.4 / 45.5 / 16.2 |
| sign in (bcrypt) | 8 | 39.1 ±1% | 34.3 ±9% | n/a | **1.14×** | n/a | 204 / 226 / n/a | 316 / 381 / n/a |

"n/a": PostgREST has no sign-in, no `expand` and no `/api/health` on its API port (its admin port's
`/live` stands in for health). p50 and p99 are in milliseconds.

### slonbase against itself

| Scenario | slonbase 2.2 | 2.2, run again at the end | slonbase 1.9 | 2.2 / 1.9 | 2.2, async commit | 2.2, no compression |
|---|--:|--:|--:|--:|--:|--:|
| health | 44,316 ±2% | 43,729 ±1% | 42,248 ±4% | **1.05×** | 45,273 ±2% | 46,077 ±3% |
| view one record | 19,965 ±3% | 19,230 ±2% | 19,820 ±1% | **1.01×** | 19,093 ±4% | 19,213 ±1% |
| view one record (1 client) | 5,765 ±3% | 5,340 ±3% | 5,299 ±1% | **1.09×** | 5,165 ±7% | 5,383 ±3% |
| list 30 | 7,814 ±1% | 7,473 ±3% | 7,635 ±2% | **1.02×** | 7,194 ±4% | 8,321 ±2% |
| list 30, sorted | 7,117 ±3% | 6,741 ±1% | 6,919 ±1% | **1.03×** | 6,566 ±3% | 7,380 ±2% |
| list 30, filtered | 7,100 ±2% | 6,790 ±3% | 6,582 ±2% | **1.08×** | 6,715 ±1% | 7,168 ±2% |
| list 30, expand author | 3,906 ±1% | 3,880 ±1% | 3,771 ±4% | **1.04×** | 3,783 ±0% | 4,037 ±0% |
| list 30, signed in | 6,321 ±2% | 6,070 ±3% | 6,200 ±4% | **1.02×** | 5,951 ±2% | 6,672 ±1% |
| create record | 7,424 ±3% | 6,927 ±4% | 7,158 ±2% | **1.04×** | 8,722 ±3% | 7,122 ±1% |
| sign in (bcrypt) | 39.1 ±1% | 40.1 ±1% | 39.1 ±1% | **1.00×** | 37.8 ±2% | 38.8 ±2% |

"async commit" is slonbase with PostgreSQL's `synchronous_commit = off` (see below); "no
compression" is `--no-compression`, which makes slonbase's lists comparable with PostgREST's,
which doesn't compress. 2.2 against 1.9 is within the run-to-run spread (the end-of-run repeat of 2.2
shows how far one machine drifts in half an hour), so 2.2 is not slower than 1.9 where 1.9 was
measured.

### Footprint

| | slonbase 2.2.0 | PocketBase 0.40.6 | PostgREST 16.5 |
|---|--:|--:|--:|
| Binary on disk | 21.3 MB | 32.0 MB | 19.6 MB |
| Download (release archive) | 9.8 MB (.tar.gz) | 12.2 MB (.zip) | 4.3 MB (.tar.xz) |
| Start to first healthy answer | 25 ms | 27 ms | 132 ms |
| Resident memory, idle (after seeding) | 40 MB | 41 MB | 15 MB |
| Peak resident memory while seeding 10,000 posts | 50 MB | 43 MB | n/a (seeded through slonbase) |
| Peak resident memory, whole run | 64 MB | 1343 MB | 99 MB |
| Seeding 10,000 posts through the API | 6.1 s | 4.3 s | (same data, seeded through slonbase) |

Resident memory is the server process only. slonbase and PostgREST keep their data in a
PostgreSQL server, whose memory isn't counted here (one server is shared by every instance);
PocketBase's SQLite runs inside its process.

## Reading the numbers

- **Lists, filters, expand:** slonbase answers 4.9 times as many list requests per second as
  PocketBase (7,814 against 1,610; 4.5 times with a sort, 3.7 with `expand`, 4.1 signed in), at
  about an eighth of its p99 latency (7.8 ms against 67 ms). Filtered lists are where the
  difference is largest, 68 times (7,100 against 105): PostgreSQL uses the index on `views` and plans the
  combined condition, while PocketBase's query takes about 0.3 s per request under this load.
- **Against PostgREST,** slonbase is 2.1 times as fast on lists (3,733), 3.8 times on single
  records (5,197) and 1.9 times on creates (3,828). PostgREST is the leanest of the three
  (a 15 MB resident process, a 4.5 MB download) and does the least: no sign-in, files, realtime or
  dashboard. Its lists are built inside PostgreSQL, on the same 3 CPUs the server uses, which is part of
  why it doesn't pull ahead here; on a machine where the database has cores of its own the gap may
  differ. See [PostgREST](#postgrest) for how it was set up.
- **Single records:** 1.7 times PocketBase's throughput with 32 clients (19,965 against 11,484)
  and 1.8 times with one (5,765 against 3,188), at about the same median (1.5 ms against 1.4 ms) and a
  sixth of its p99 (3.7 ms against 23 ms).
- **Writes:** 1.8 times as fast as PocketBase with PostgreSQL's safe defaults (7,424 creates a
  second against 4,051), 2.2 times with asynchronous commits (see durability below).
- **Compression:** hey accepts gzip, so lists are measured compressed (2.2 KB instead of 14.5 KB a
  page); without compression, slonbase's lists are roughly 10% faster here, and the fairer figure when
  comparing with PostgREST, which doesn't compress ([what changed in 1.7](#what-changed-in-17)).
- **Durability:** with PostgreSQL's defaults, every commit waits for the WAL to reach the disk
  (`synchronous_commit = on`). PocketBase's SQLite runs in WAL mode with `synchronous =
  NORMAL`, which doesn't wait for each commit, so a power loss can drop its last transactions
  (without corrupting the database). With `synchronous_commit = off`, PostgreSQL gives that
  same guarantee. Which one to use is a choice between durability and write latency; slonbase
  keeps PostgreSQL's safe default.
- **Sign-in** is dominated by bcrypt (cost 10 on both), so the two are about equal (39 against 34).
- **Memory:** slonbase idles at 40 MB, about as much as PocketBase (41 MB), and peaked at 64 MB over
  the whole run. PocketBase peaked at 1.3 GB over the whole run, though it held 44 MB after seeding, so the
  growth came from the request load (the 105-a-second filtered lists, in all likelihood: it did not
  go back down). PostgreSQL's memory, shared by every slonbase instance, is not counted, and
  PocketBase needs no separate server. Both are real costs and benefits of the two designs.
- **Caveats:** one machine with shared CPUs, the database and the server on the same host, and
  loopback networking. A production setup with PostgreSQL on its own machine adds a network round
  trip to every slonbase query but gives the database its own CPUs, memory and disks, which
  PocketBase's embedded SQLite can't use; and several slonbase instances can share one database.
  Results of other machines will differ; compare within a table, and repeat the runs yourself
  ([below](#running-them)).

## PostgREST

PostgREST is a REST layer over a PostgreSQL schema, nothing more: no sign-in or user management
(it verifies JWTs you issue), no files, no realtime, no dashboard. It is in the comparison because
it is the usual answer to "I have PostgreSQL and want an API", and because it answers the question
of how much of slonbase's speed comes from PostgreSQL itself. How it was set up here:

- it reads and writes the tables slonbase created (`slonbase.posts`, `slonbase.writes`), as an
  `anon` role with `SELECT` and `INSERT`; the `writes` table loses slonbase's realtime trigger
  (PostgREST has none) and gets a default `id`, since PostgREST doesn't generate record ids;
- `db-pool = 16`, as slonbase's default pool; nothing else was tuned (no pre-request function, no
  schema cache tweaks, no JWT). A tuned PostgREST may do better than the table shows;
- it runs on the same 3 CPUs as PostgreSQL, which also does the JSON building for it, so the
  database and the API layer compete for the same cores, as they do for slonbase.

## What changed in 1.9

**Health checks.** Each check ran `SELECT 1` on a pooled connection. The probe's outcome is now
kept for a second, and requests that arrive while a probe runs wait for it rather than starting
their own, so a burst costs one query a second. An outage still shows within a second.

**Single-record reads.** Profiled with callgrind after warm-up, a read took about 172,000
instructions; an eighth went to building the column list with three `format!`s per field, and
more to turning the row into a record value and back into JSON. The column list is now written
into one buffer, and a record with nothing to change it on the way out (no view or enrich
hooks, no `expand`) is written straight from its row, as lists already were: 136,600
instructions. `tests/list_output.rs` checks both paths give the same bytes.

## What changed in 1.8

**Creates in one round trip.** A create sent BEGIN, its INSERT and COMMIT. When nothing can
undo the INSERT once it ran (no execute hook around it, no create rule checked against the new
record), it now runs on its own, which PostgreSQL commits the same way. pgbench on the same
machine, 32 clients: 16,911 BEGIN/INSERT/COMMIT transactions per second, 28,406 single INSERTs.

## What changed in 1.7

**Writes.** Before 1.7 the realtime trigger on each table called `pg_notify` in every write.
PostgreSQL serializes the commits of transactions that notified (they take a global lock on
the notification queue), so writes committed one at a time however many clients sent them.
pgbench on the same machine, 32 clients:

| Insert into a collection table | tps |
|---|--:|
| with the realtime trigger and its `pg_notify` (1.6) | 352 |
| with the trigger, without the notification (1.7) | 7,108 |
| without the trigger | 28,406 |

1.7 reads new events from the outbox by transaction snapshot instead, and tells other
instances with one coalesced notification outside the writes (details in the
[design notes](../docs/design.md#phase-3-realtime-and-multi-instance-sync)). Through the API,
creates went from 1,899 to 3,121 per second, and to 4,012 with asynchronous commits.

**Compression.** JSON answers are gzipped in one pass with a compressor kept per thread, at
level 1 on zlib-rs (25 µs for a page of 30); the first try, tower-http's streaming encoder at
its defaults, cost lists 22% of their throughput here, this one about 8%.

## What changed in 1.3

1.2.0 against 1.3.0 is the same machine and data: lists are 5 to 8 times faster, single records
2.85 times. Where the time went, and what was done (details in the
[design notes](../docs/design.md#performance)):

| Change | What it removed | Measured effect |
|---|---|---|
| mimalloc as the allocator | musl's allocator serializing every allocation of the static release binaries | lists without a total 4.8×, single records 1.85× |
| Prepared statements for repeating SQL | PostgreSQL parsing and planning every query | a lookup by id 2.7× faster in PostgreSQL (pgbench) |
| Totals kept until data changes | `count(*)` over every matching row on each page (2.4 ms, against 0.3 ms for the page) | with the statements: lists with a total about 2.9× |
| Lists written straight from the rows | a map per record, the hashing of its keys and a copy into the response (a third of a list's instructions) | the same bytes, less CPU per list |

The dashboard's files are now cached for good under the build's hash, and its page preloads
every module. Through 100 ms of latency, a repeat visit shows the sign-in form in 340 ms with
2 requests, against 940 ms and 33 requests (31 of them revalidations) with 1.2; a first visit
over HTTP/1.1, which fetches 6 files at a time, takes about as long as before, and behind an
HTTP/2 proxy the preloads come in parallel.

## Running them

```sh
npm ci
go install github.com/rakyll/hey@latest
(cd .. && cargo build --release)    # or SLONBASE_BIN=/path/to/a/release/binary
# PostgreSQL's processes (to pin them with the server): PG_PIDS="$(pgrep -x postgres)"
DATABASE_URL='postgres://...an empty database...' PG_PIDS="$(pgrep -x postgres)" ./run.sh slonbase
POCKETBASE_BIN=/path/to/pocketbase ./run.sh pocketbase
# PostgREST: another empty database; slonbase seeds it, then PostgREST serves it
DATABASE_URL='postgres://...an empty database...' SLONBASE_BIN=/path/to/slonbase POSTGREST_BIN=/path/to/postgrest ./run.sh postgrest
# optional: slonbase with asynchronous commits (on a fresh database)
#   ALTER DATABASE <db> SET synchronous_commit = off;
BENCH_NAME=slonbase-async DATABASE_URL=... ./run.sh slonbase
# optional: without compression, or the previous release
SLONBASE_ARGS=--no-compression BENCH_NAME=slonbase-nocompress DATABASE_URL=... ./run.sh slonbase
BENCH_NAME=slonbase-previous SLONBASE_BIN=/path/to/previous/slonbase DATABASE_URL=... ./run.sh slonbase
node report.mjs
```

Settings: `BENCH_RUNS` (3), `BENCH_DURATION` (10s), `BENCH_WARMUP` (2s), `BENCH_POSTS`
(10000), `SERVER_CPUS` (0-2), `LOAD_CPUS` (3), `HEY_BIN`, `SLONBASE_ARGS`. Results go to `results/`
(`<name>.json` with the medians and the footprint, `<name>.raw.jsonl` with every run). The
runs behind the 2.2 tables are kept in [`published/2.2.0/`](published/2.2.0).

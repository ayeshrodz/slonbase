# Benchmarks

slonbase and PocketBase on the same workloads, through the same HTTP API: `seed.mjs` creates
the same data on either server with the official SDK (100 authors, 10,000 posts with an index on
`views`, a user), and `run.sh` measures each scenario with [hey](https://github.com/rakyll/hey).

## Results

slonbase 1.7.0 and 1.6.0 (static musl release binaries; 1.6.0 as published), PostgreSQL 16
with default settings, and PocketBase 0.40.5 (the official release binary, default settings),
on a 4-CPU cloud VM (Intel Xeon @ 2.80GHz, 15 GB): the server (and PostgreSQL) pinned to 3
CPUs, the load generator to the fourth. Each scenario runs 3 times for 10 s after a 2 s
warm-up; the table shows the medians, and ± is half the spread between the runs. (This VM was
slower than the one 1.3 was measured on: compare within a table, not across them.)

| Scenario | Clients | slonbase 1.7 req/s | slonbase 1.6 req/s | 1.7 / 1.6 | slonbase 1.7, async commit | PocketBase req/s | slonbase / PocketBase | async / PocketBase | p50 ms (slonbase / PB) | p99 ms (slonbase / PB) |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| health | 32 | 15,563 ±4% | 16,840 ±3% | 0.92× | 16,402 ±4% | 19,665 ±2% | 0.79× | 0.83× | 1.90 / 1.20 | 5.40 / 9.40 |
| view one record | 32 | 10,128 ±8% | 10,736 ±2% | 0.94× | 10,487 ±4% | 8,299 ±5% | 1.22× | 1.26× | 3.00 / 2.20 | 7.30 / 30.8 |
| view one record (1 client) | 1 | 1,938 ±5% | 2,106 ±13% | 0.92× | 2,205 ±10% | 2,059 ±4% | 0.94× | 1.07× | 0.50 / 0.40 | 1.10 / 1.80 |
| list 30 | 32 | 4,871 ±3% | 5,590 ±3% | 0.87× | 4,979 ±4% | 1,030 ±1% | 4.73× | 4.83× | 6.20 / 25.7 | 13.4 / 108.1 |
| list 30, sorted | 32 | 4,240 ±3% | 4,979 ±3% | 0.85× | 4,389 ±6% | 997 ±2% | 4.25× | 4.40× | 7.20 / 27.1 | 15.2 / 113.3 |
| list 30, filtered | 32 | 4,304 ±1% | 4,988 ±5% | 0.86× | 4,475 ±9% | 71.9 ±2% | 59.84× | 62.22× | 7.10 / 441.5 | 15.5 / 684.6 |
| list 30, expand author | 32 | 2,254 ±1% | 2,632 ±6% | 0.86× | 2,275 ±9% | 733 ±2% | 3.08× | 3.10× | 14.1 / 35.0 | 22.1 / 154.0 |
| list 30, signed in | 32 | 3,705 ±1% | 4,000 ±6% | 0.93× | 3,678 ±4% | 977 ±5% | 3.79× | 3.77× | 8.50 / 28.1 | 15.0 / 102.0 |
| create record | 32 | 3,121 ±1% | 1,899 ±5% | **1.64×** | 4,012 ±2% | 2,708 ±6% | 1.15× | 1.48× | 9.70 / 7.00 | 19.4 / 60.5 |
| sign in (bcrypt) | 8 | 39.7 ±0% | 40.2 ±1% | 0.99× | 40.1 ±1% | 36.1 ±7% | 1.10× | 1.11× | 198.3 / 217.0 | 367.9 / 353.7 |

"async commit" is slonbase with PostgreSQL's `synchronous_commit = off` (see below).

## Reading the numbers

- **Writes:** 1.7 creates records 1.64 times as fast as 1.6, and now faster than PocketBase
  with PostgreSQL's safe defaults too (1.15×; 1.48× with asynchronous commits). See
  [what changed in 1.7](#what-changed-in-17).
- **Lists, filters, expand:** slonbase answers 3.1 to 4.7 times as many requests per second as
  PocketBase (60 times for the filtered list), at about a seventh of its p99 latency (13 to
  22 ms against 102 to 154 ms). Filtered lists are where the difference is largest: PostgreSQL
  uses the index on `views` and plans the combined condition, while PocketBase's query takes
  about 0.4 s per request under this load.
- **1.7 against 1.6 on lists** (0.85–0.93×): hey accepts gzip, and 1.7 gzips JSON (a page of
  30 is 2.2 KB instead of 14.5 KB). On this machine the server and PostgreSQL share 3 CPUs, so
  the compression shows; with `--no-compression`, or a client that doesn't take gzip, lists
  are as fast as in 1.6 (alternating runs of each: within 3%). Outside a loopback benchmark,
  six times fewer bytes on the wire usually save more time than the 25 µs they cost.
- **Single records:** slonbase is ahead with many clients (1.22×), and about even with one.
- **Health:** PocketBase answers its health check faster: slonbase's runs a query, so it
  reports a database that's down. (1.7 against 1.6 here, 0.92×, is noise: alternating runs of
  the two were within 2.5%.)
- **Durability:** with PostgreSQL's defaults, every commit waits for the WAL to reach the disk
  (`synchronous_commit = on`). PocketBase's SQLite runs in WAL mode with `synchronous =
  NORMAL`, which doesn't wait for each commit, so a power loss can drop its last transactions
  (without corrupting the database). With `synchronous_commit = off`, PostgreSQL gives that
  same guarantee. Which one to use is a choice between durability and write latency; slonbase
  keeps PostgreSQL's safe default.
- **Sign-in** is dominated by bcrypt (cost 10 on both), so the two are about equal.
- **Caveats:** one machine with shared CPUs, the database and the server on the same host, and
  loopback networking. A production setup with PostgreSQL on its own machine adds a network round
  trip to every slonbase query but gives the database its own CPUs, memory and disks, which
  PocketBase's embedded SQLite can't use; and several slonbase instances can share one database.

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
go install github.com/rakyll/hey@v0.1.4
(cd .. && cargo build --release)    # or SLONBASE_BIN=/path/to/a/release/binary
# PostgreSQL's processes (to pin them with the server): PG_PIDS="$(pgrep -u postgres)"
DATABASE_URL='postgres://...an empty database...' PG_PIDS="$(pgrep -u postgres)" ./run.sh slonbase
POCKETBASE_BIN=/path/to/pocketbase ./run.sh pocketbase
# optional: slonbase with asynchronous commits (on a fresh database)
#   ALTER DATABASE <db> SET synchronous_commit = off;
BENCH_NAME=slonbase-async DATABASE_URL=... ./run.sh slonbase
# optional: the previous release, for the "vs previous" column
BENCH_NAME=slonbase-previous SLONBASE_BIN=/path/to/previous/slonbase DATABASE_URL=... ./run.sh slonbase
node report.mjs
```

Settings: `BENCH_RUNS` (3), `BENCH_DURATION` (10s), `BENCH_WARMUP` (2s), `BENCH_POSTS`
(10000), `SERVER_CPUS` (0-2), `LOAD_CPUS` (3), `HEY_BIN`. Results go to `results/` (not
committed; this file holds the published numbers).

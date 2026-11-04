# Benchmarks

slonbase and PocketBase on the same workloads, through the same HTTP API: `seed.mjs` creates
the same data on either server with the official SDK (100 authors, 10,000 posts with an index on
`views`, a user), and `run.sh` measures each scenario with [hey](https://github.com/rakyll/hey).

## Results

slonbase 1.9.0 and 1.8.0 (static musl release binaries; 1.8.0 as published), PostgreSQL 16
with default settings, and PocketBase 0.40.5 (the official release binary, default settings),
on a 4-CPU cloud VM (Intel Xeon @ 2.80GHz, 15 GB): the server (and PostgreSQL) pinned to 3
CPUs, the load generator to the fourth. Each scenario runs 3 times for 10 s after a 2 s
warm-up; the table shows the medians, and ± is half the spread between the runs. (Compare
within a table: the VM's speed varies from day to day, and was about 1.6 times as fast as for
1.8's table.)

| Scenario | Clients | slonbase 1.9 req/s | slonbase 1.8 req/s | 1.9 / 1.8 | slonbase 1.9, async commit | PocketBase req/s | slonbase / PocketBase | async / PocketBase | p50 ms (slonbase / PB) | p99 ms (slonbase / PB) |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| health | 32 | 34,782 ±2% | 24,238 ±2% | **1.43×** | 34,355 ±3% | 30,569 ±7% | 1.14× | 1.12× | 0.80 / 0.80 | 2.80 / 5.80 |
| view one record | 32 | 15,375 ±7% | 15,097 ±1% | 1.02× | 14,850 ±1% | 13,311 ±5% | 1.16× | 1.12× | 2.00 / 1.30 | 5.30 / 22.0 |
| view one record (1 client) | 1 | 3,946 ±2% | 3,666 ±1% | 1.08× | 3,825 ±11% | 2,503 ±14% | 1.58× | 1.53× | 0.20 / 0.30 | 0.70 / 1.20 |
| list 30 | 32 | 6,604 ±3% | 6,920 ±3% | 0.95× | 6,271 ±0% | 1,678 ±8% | 3.94× | 3.74× | 4.70 / 15.4 | 9.70 / 69.5 |
| list 30, sorted | 32 | 5,767 ±4% | 6,183 ±1% | 0.93× | 5,480 ±2% | 1,614 ±2% | 3.57× | 3.40× | 5.30 / 16.7 | 10.9 / 67.1 |
| list 30, filtered | 32 | 6,192 ±4% | 6,024 ±0% | 1.03× | 5,671 ±5% | 74.0 ±2% | 83.71× | 76.65× | 5.00 / 432.4 | 9.90 / 652.6 |
| list 30, expand author | 32 | 3,240 ±2% | 3,137 ±3% | 1.03× | 3,021 ±4% | 1,123 ±6% | 2.88× | 2.69× | 9.70 / 22.6 | 16.0 / 101.9 |
| list 30, signed in | 32 | 5,049 ±1% | 5,049 ±5% | 1.00× | 4,999 ±2% | 1,596 ±3% | 3.16× | 3.13× | 6.20 / 16.9 | 11.3 / 61.2 |
| create record | 32 | 5,304 ±3% | 5,873 ±6% | 0.90× | 6,973 ±1% | 4,277 ±5% | 1.24× | 1.63× | 5.70 / 4.10 | 12.0 / 41.6 |
| sign in (bcrypt) | 8 | 42.1 ±1% | 44.6 ±4% | 0.94× | 43.6 ±1% | 40.1 ±1% | 1.05× | 1.09× | 186.5 / 193.7 | 308.1 / 317.0 |

"async commit" is slonbase with PostgreSQL's `synchronous_commit = off` (see below).

1.8 and 1.9 ran one after the other here, so their column also carries the VM's drift. Run
alternately against the same database (seven rounds each, in both orders), 1.9 answers 1.47
times as many health checks and 1.13 times as many single-record reads with 32 clients as 1.8,
and is even with it on lists (1.00× plain, 1.01× sorted) and creates (1.01×): the 0.90 to 0.95
above is drift, not a change.

## Reading the numbers

- **Health:** ahead of PocketBase now (1.14×; it was 0.78× with 1.8): slonbase still checks
  the database, but at most once a second, shared by every request in that second. See
  [what changed in 1.9](#what-changed-in-19).
- **Single records:** ahead with many clients (1.16×) and with one (1.58×).
- **Writes:** 1.37 times as fast as PocketBase in 1.8's table and 1.24 times here with
  PostgreSQL's safe defaults, 1.63 times with asynchronous commits.
- **Lists, filters, expand:** slonbase answers 2.9 to 3.9 times as many requests per second as
  PocketBase (84 times for the filtered list), at about a sixth of its p99 latency (10 to
  16 ms against 61 to 102 ms). Filtered lists are where the difference is largest: PostgreSQL
  uses the index on `views` and plans the combined condition, while PocketBase's query takes
  about 0.43 s per request under this load.
- **Compression:** hey accepts gzip, so lists are measured compressed (2.2 KB instead of
  14.5 KB a page); on this loopback setup that costs them about 8% against `--no-compression`
  ([what changed in 1.7](#what-changed-in-17)).
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

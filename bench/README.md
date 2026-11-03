# Benchmarks

slonbase and PocketBase on the same workloads, through the same HTTP API: `seed.mjs` creates
the same data on either server with the official SDK (100 authors, 10,000 posts with an index on
`views`, a user), and `run.sh` measures each scenario with [hey](https://github.com/rakyll/hey).

## Results

slonbase 1.8.0 and 1.7.0 (static musl release binaries; 1.7.0 as published), PostgreSQL 16
with default settings, and PocketBase 0.40.5 (the official release binary, default settings),
on a 4-CPU cloud VM (Intel Xeon @ 2.80GHz, 15 GB): the server (and PostgreSQL) pinned to 3
CPUs, the load generator to the fourth. Each scenario runs 3 times for 10 s after a 2 s
warm-up; the table shows the medians, and ± is half the spread between the runs. (Compare
within a table: the VM's speed varies from day to day.)

| Scenario | Clients | slonbase 1.8 req/s | slonbase 1.7 req/s | 1.8 / 1.7 | slonbase 1.8, async commit | PocketBase req/s | slonbase / PocketBase | async / PocketBase | p50 ms (slonbase / PB) | p99 ms (slonbase / PB) |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| health | 32 | 14,561 ±1% | 12,684 ±10% | 1.15× | 15,190 ±7% | 18,663 ±4% | 0.78× | 0.81× | 2.00 / 1.20 | 5.80 / 8.80 |
| view one record | 32 | 9,671 ±2% | 9,216 ±6% | 1.05× | 9,850 ±3% | 7,969 ±1% | 1.21× | 1.24× | 3.10 / 2.20 | 7.90 / 33.2 |
| view one record (1 client) | 1 | 1,773 ±7% | 1,648 ±7% | 1.08× | 1,904 ±8% | 1,958 ±4% | 0.91× | 0.97× | 0.50 / 0.40 | 1.10 / 2.10 |
| list 30 | 32 | 4,444 ±3% | 4,481 ±4% | 0.99× | 4,434 ±4% | 1,007 ±4% | 4.41× | 4.40× | 6.90 / 26.9 | 14.7 / 110.4 |
| list 30, sorted | 32 | 3,987 ±1% | 4,079 ±8% | 0.98× | 3,979 ±5% | 957 ±1% | 4.17× | 4.16× | 7.70 / 28.5 | 16.5 / 121.6 |
| list 30, filtered | 32 | 3,900 ±4% | 3,779 ±3% | 1.03× | 3,864 ±6% | 70.1 ±8% | 55.63× | 55.12× | 7.90 / 457.6 | 16.4 / 747.1 |
| list 30, expand author | 32 | 2,028 ±5% | 1,968 ±4% | 1.03× | 2,058 ±1% | 723 ±6% | 2.81× | 2.85× | 15.5 / 35.8 | 25.6 / 156.8 |
| list 30, signed in | 32 | 3,438 ±2% | 3,280 ±6% | 1.05× | 3,588 ±7% | 919 ±3% | 3.74× | 3.90× | 9.10 / 29.3 | 16.6 / 105.4 |
| create record | 32 | 3,831 ±5% | 2,929 ±5% | **1.31×** | 4,811 ±4% | 2,795 ±4% | 1.37× | 1.72× | 8.00 / 6.90 | 15.5 / 62.3 |
| sign in (bcrypt) | 8 | 39.1 ±2% | 39.2 ±2% | 1.00× | 38.6 ±2% | 36.2 ±6% | 1.08× | 1.07× | 205.6 / 217.5 | 360.2 / 338.5 |

"async commit" is slonbase with PostgreSQL's `synchronous_commit = off` (see below).

## Reading the numbers

- **Writes:** 1.8 creates records 1.31 times as fast as 1.7 here (1.18 times in alternating
  runs of the two against the same database, the steadier measure), 1.37 times as fast as
  PocketBase with PostgreSQL's safe defaults, and 1.72 times with asynchronous commits. See
  [what changed in 1.8](#what-changed-in-18).
- **Lists, filters, expand:** slonbase answers 2.8 to 4.4 times as many requests per second as
  PocketBase (56 times for the filtered list), at about a seventh of its p99 latency (15 to
  26 ms against 105 to 157 ms). Filtered lists are where the difference is largest: PostgreSQL
  uses the index on `views` and plans the combined condition, while PocketBase's query takes
  about 0.45 s per request under this load.
- **Everything but creates** is within the run-to-run spread of 1.7 (health's 1.15× too:
  1.7's runs spread ±10%).
- **Compression:** hey accepts gzip, so lists are measured compressed (2.2 KB instead of
  14.5 KB a page); on this loopback setup that costs them about 8% against `--no-compression`
  ([what changed in 1.7](#what-changed-in-17)).
- **Single records:** slonbase is ahead with many clients (1.21×), and about even with one.
- **Health:** PocketBase answers its health check faster: slonbase's runs a query, so it
  reports a database that's down.
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

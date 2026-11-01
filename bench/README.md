# Benchmarks

slonbase and PocketBase on the same workloads, through the same HTTP API: `seed.mjs` creates
the same data on either server with the official SDK (100 authors, 10,000 posts with an index on
`views`, a user), and `run.sh` measures each scenario with [hey](https://github.com/rakyll/hey).

## Results

slonbase 1.3.0 and 1.2.0 (the static musl release binaries, as published), PostgreSQL 16 with
default settings, and PocketBase 0.40.5 (the official release binary, default settings), on a
4-CPU cloud VM (Intel Xeon @ 2.80GHz, 15 GB): the server (and PostgreSQL) pinned to 3 CPUs,
the load generator to the fourth. Each scenario runs 3 times for 10 s after a 2 s warm-up; the
table shows the medians, and ± is half the spread between the runs.

| Scenario | Clients | slonbase 1.3 req/s | slonbase 1.2 req/s | 1.3 / 1.2 | slonbase 1.3, async commit | PocketBase req/s | slonbase / PocketBase | async / PocketBase | p50 ms (slonbase / PB) | p99 ms (slonbase / PB) |
|---|--:|--:|--:|--:|--:|--:|--:|--:|--:|--:|
| health | 32 | 21,891 ±4% | 14,439 ±6% | 1.52× | 20,721 ±2% | 25,243 ±2% | 0.87× | 0.82× | 1.30 / 0.90 | 4.20 / 7.10 |
| view one record | 32 | 13,677 ±3% | 4,800 ±7% | 2.85× | 13,416 ±2% | 10,083 ±2% | 1.36× | 1.33× | 2.20 / 1.70 | 5.90 / 27.4 |
| view one record (1 client) | 1 | 2,764 ±6% | 1,605 ±4% | 1.72× | 2,703 ±8% | 2,476 ±5% | 1.12× | 1.09× | 0.30 / 0.30 | 0.80 / 1.50 |
| list 30 | 32 | 6,863 ±2% | 872 ±3% | 7.87× | 6,659 ±4% | 1,307 ±4% | 5.25× | 5.09× | 4.50 / 20.4 | 9.30 / 81.6 |
| list 30, sorted | 32 | 5,857 ±2% | 907 ±5% | 6.46× | 5,768 ±2% | 1,235 ±3% | 4.74× | 4.67× | 5.30 / 21.2 | 10.9 / 86.8 |
| list 30, filtered | 32 | 5,965 ±4% | 858 ±6% | 6.95× | 6,065 ±1% | 83.3 ±3% | 71.63× | 72.84× | 5.20 / 396.5 | 10.6 / 533.8 |
| list 30, expand author | 32 | 3,079 ±2% | 589 ±5% | 5.23× | 2,952 ±6% | 931 ±3% | 3.31× | 3.17× | 10.2 / 28.2 | 17.2 / 113.7 |
| list 30, signed in | 32 | 5,048 ±1% | 811 ±1% | 6.23× | 5,182 ±0% | 1,249 ±1% | 4.04× | 4.15× | 6.20 / 22.1 | 11.2 / 75.7 |
| create record | 32 | 2,204 ±5% | 1,910 ±6% | 1.15× | 4,520 ±1% | 3,622 ±5% | 0.61× | 1.25× | 13.9 / 5.10 | 24.6 / 51.1 |
| sign in (bcrypt) | 8 | 40.7 ±2% | 40.6 ±0% | 1.00× | 40.8 ±0% | 36.0 ±2% | 1.13× | 1.13× | 197.0 / 218.7 | 300.3 / 360.4 |

"async commit" is slonbase with PostgreSQL's `synchronous_commit = off` (see below).

## Reading the numbers

- **Lists, filters, expand:** slonbase answers 3.3 to 5.3 times as many requests per second as
  PocketBase, at about a tenth of its p99 latency (9 to 17 ms against 76 to 114 ms). Filtered
  lists are where the difference is largest: PostgreSQL uses the index on `views` and plans the
  combined condition, while PocketBase's query takes about 0.4 s per request under this load.
- **Single records:** slonbase is ahead too, with many clients (1.36×) and with one (1.12×, both
  at 0.3 ms); before 1.3, PocketBase's embedded SQLite was faster for one client.
- **Health:** PocketBase answers its health check faster: slonbase's runs a query, so it
  reports a database that's down.
- **Writes:** with PostgreSQL's defaults, every commit waits for the WAL to reach the disk
  (`synchronous_commit = on`). PocketBase's SQLite runs in WAL mode with `synchronous = NORMAL`,
  which doesn't wait for each commit, so a power loss can drop its last transactions (without
  corrupting the database). With `synchronous_commit = off`, PostgreSQL gives that same
  guarantee, and slonbase writes faster than PocketBase (1.25×; 0.61× with the default). Which
  one to use is a choice between durability and write latency; slonbase keeps PostgreSQL's
  safe default.
- **Sign-in** is dominated by bcrypt (cost 10 on both), so the two are about equal.
- **Caveats:** one machine with shared CPUs, the database and the server on the same host, and
  loopback networking. A production setup with PostgreSQL on its own machine adds a network round
  trip to every slonbase query but gives the database its own CPUs, memory and disks, which
  PocketBase's embedded SQLite can't use; and several slonbase instances can share one database.

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

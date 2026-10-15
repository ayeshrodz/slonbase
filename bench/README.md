# Benchmarks

slonbase and PocketBase on the same workloads, through the same HTTP API: `seed.mjs` creates
the same data on either server with the official SDK (100 authors, 10,000 posts with an index on
`views`, a user), and `run.sh` measures each scenario with [hey](https://github.com/rakyll/hey).

## Results

slonbase 0.1.0 (release build, PostgreSQL 16, default settings) and PocketBase 0.40.5 (the
official release binary, default settings), on a 4-CPU cloud VM (Intel Xeon @ 2.80GHz, 15 GB):
the server (and PostgreSQL) pinned to 3 CPUs, the load generator to the fourth. Each scenario
runs 3 times for 10 s after a 2 s warm-up; the table shows the medians, and ± is half the
spread between the runs.

| Scenario | Clients | slonbase req/s | slonbase, async commit | PocketBase req/s | slonbase / PocketBase | async / PocketBase | p50 ms (slonbase / PB) | p99 ms (slonbase / PB) |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| health | 32 | 15,091 ±6% | 15,876 ±2% | 15,827 ±9% | 0.95× | 1.00× | 1.90 / 1.50 | 6.00 / 11.0 |
| view one record | 32 | 6,032 ±4% | 5,712 ±5% | 6,360 ±7% | 0.95× | 0.90× | 5.00 / 2.80 | 12.2 / 39.1 |
| view one record (1 client) | 1 | 1,141 ±5% | 1,171 ±6% | 1,540 ±7% | 0.74× | 0.76× | 0.80 / 0.50 | 2.60 / 2.90 |
| list 30 | 32 | 1,088 ±2% | 1,281 ±16% | 928 ±4% | 1.17× | 1.38× | 28.4 / 29.2 | 53.8 / 123.4 |
| list 30, sorted | 32 | 1,038 ±6% | 1,259 ±9% | 878 ±5% | 1.18× | 1.43× | 29.8 / 30.8 | 55.3 / 124.5 |
| list 30, filtered | 32 | 836 ±12% | 925 ±4% | 60.2 ±4% | 13.89× | 15.37× | 36.5 / 534.7 | 75.1 / 711.4 |
| list 30, expand author | 32 | 709 ±1% | 759 ±4% | 654 ±4% | 1.08× | 1.16× | 44.2 / 38.5 | 70.5 / 176.3 |
| list 30, signed in | 32 | 830 ±2% | 1,026 ±2% | 796 ±7% | 1.04× | 1.29× | 37.4 / 34.2 | 63.8 / 127.0 |
| create record | 32 | 1,417 ±4% | 2,220 ±5% | 2,350 ±5% | 0.60× | 0.94× | 21.9 / 8.30 | 36.1 / 72.8 |
| sign in (bcrypt) | 8 | 37.8 ±1% | 37.1 ±3% | 35.1 ±10% | 1.08× | 1.06× | 208.0 / 223.7 | 332.0 / 349.4 |

"async commit" is slonbase with PostgreSQL's `synchronous_commit = off` (see below).

## Reading the numbers

- **Lists, filters, expand:** slonbase answers more requests per second and keeps a much
  lower tail latency (p99 roughly half of PocketBase's or less). Filtered lists are where the
  difference is largest: PostgreSQL uses the index on `views` and plans the combined condition,
  while PocketBase's query takes about 0.5 s per request under this load.
- **Single record, one client:** PocketBase is faster (0.5 ms against 0.8 ms). SQLite runs inside
  the PocketBase process, while every slonbase query is a round trip to PostgreSQL over a socket;
  with many clients the difference disappears (0.95×).
- **Writes:** with PostgreSQL's defaults, every commit waits for the WAL to reach the disk
  (`synchronous_commit = on`). PocketBase's SQLite runs in WAL mode with `synchronous = NORMAL`,
  which doesn't wait for each commit, so a power loss can drop its last transactions (without
  corrupting the database). With `synchronous_commit = off`, PostgreSQL gives that same
  guarantee, and slonbase writes as fast as PocketBase (0.94×). Which one to use is a choice
  between durability and write latency; slonbase keeps PostgreSQL's safe default.
- **Sign-in** is dominated by bcrypt (cost 10 on both), so the two are about equal.
- **Caveats:** one machine with shared CPUs, the database and the server on the same host, and
  loopback networking. A production setup with PostgreSQL on its own machine adds a network round
  trip to every slonbase query but gives the database its own CPUs, memory and disks, which
  PocketBase's embedded SQLite can't use; and several slonbase instances can share one database.

## Running them

```sh
npm ci
go install github.com/rakyll/hey@v0.1.4
(cd .. && cargo build --release)
# PostgreSQL's processes (to pin them with the server): PG_PIDS="$(pgrep -u postgres)"
DATABASE_URL='postgres://...an empty database...' PG_PIDS="$(pgrep -u postgres)" ./run.sh slonbase
POCKETBASE_BIN=/path/to/pocketbase ./run.sh pocketbase
# optional: slonbase with asynchronous commits (on a fresh database)
#   ALTER DATABASE <db> SET synchronous_commit = off;
BENCH_NAME=slonbase-async DATABASE_URL=... ./run.sh slonbase
node report.mjs
```

Settings: `BENCH_RUNS` (3), `BENCH_DURATION` (10s), `BENCH_WARMUP` (2s), `BENCH_POSTS`
(10000), `SERVER_CPUS` (0-2), `LOAD_CPUS` (3), `HEY_BIN`. Results go to `results/` (not
committed; this file holds the published numbers).

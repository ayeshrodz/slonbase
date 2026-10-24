# Benchmarks

slonbase and PocketBase on the same workloads, through the same HTTP API: `seed.mjs` creates
the same data on either server with the official SDK (100 authors, 10,000 posts with an index on
`views`, a user), and `run.sh` measures each scenario with [hey](https://github.com/rakyll/hey).

## Results

slonbase 0.4.1 (release build, PostgreSQL 16, default settings) and PocketBase 0.40.5 (the
official release binary, default settings), on a 4-CPU cloud VM (Intel Xeon @ 2.80GHz, 15 GB):
the server (and PostgreSQL) pinned to 3 CPUs, the load generator to the fourth. Each scenario
runs 3 times for 10 s after a 2 s warm-up; the table shows the medians, and ± is half the
spread between the runs.

| Scenario | Clients | slonbase req/s | slonbase, async commit | PocketBase req/s | slonbase / PocketBase | async / PocketBase | p50 ms (slonbase / PB) | p99 ms (slonbase / PB) |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| health | 32 | 18,950 ±3% | 18,213 ±6% | 18,566 ±4% | 1.02× | 0.98× | 1.60 / 1.30 | 4.40 / 9.40 |
| view one record | 32 | 7,222 ±1% | 7,294 ±6% | 7,115 ±3% | 1.02× | 1.03× | 4.20 / 2.50 | 9.60 / 33.3 |
| view one record (1 client) | 1 | 1,446 ±7% | 1,548 ±4% | 1,906 ±2% | 0.76× | 0.81× | 0.70 / 0.40 | 1.40 / 1.90 |
| list 30 | 32 | 1,557 ±2% | 1,556 ±6% | 1,043 ±1% | 1.49× | 1.49× | 20.0 / 26.1 | 36.5 / 101.8 |
| list 30, sorted | 32 | 1,516 ±3% | 1,486 ±2% | 1,026 ±0% | 1.48× | 1.45× | 20.5 / 27.0 | 37.7 / 100.8 |
| list 30, filtered | 32 | 1,175 ±3% | 1,131 ±2% | 73.0 ±5% | 16.11× | 15.50× | 26.5 / 445.2 | 47.7 / 623.3 |
| list 30, expand author | 32 | 973 ±3% | 921 ±2% | 701 ±1% | 1.39× | 1.31× | 32.4 / 35.9 | 50.5 / 161.5 |
| list 30, signed in | 32 | 1,227 ±2% | 1,271 ±5% | 947 ±6% | 1.30× | 1.34× | 25.4 / 30.0 | 41.9 / 101.5 |
| create record | 32 | 1,728 ±4% | 2,792 ±5% | 2,532 ±12% | 0.68× | 1.10× | 17.8 / 7.70 | 29.8 / 66.0 |
| sign in (bcrypt) | 8 | 40.1 ±1% | 41.2 ±0% | 36.6 ±10% | 1.09× | 1.12× | 197.2 / 211.3 | 299.9 / 346.8 |

"async commit" is slonbase with PostgreSQL's `synchronous_commit = off` (see below).

## Reading the numbers

- **Lists, filters, expand:** slonbase answers 1.3 to 1.5 times as many requests per second
  and keeps a much lower tail latency (p99 about a third of PocketBase's). Filtered lists are
  where the difference is largest: PostgreSQL uses the index on `views` and plans the combined
  condition,
  while PocketBase's query takes about 0.45 s per request under this load.
- **Single record, one client:** PocketBase is faster (0.4 ms against 0.7 ms). SQLite runs inside
  the PocketBase process, while every slonbase query is a round trip to PostgreSQL over a socket;
  with many clients the difference disappears (1.02×).
- **Writes:** with PostgreSQL's defaults, every commit waits for the WAL to reach the disk
  (`synchronous_commit = on`). PocketBase's SQLite runs in WAL mode with `synchronous = NORMAL`,
  which doesn't wait for each commit, so a power loss can drop its last transactions (without
  corrupting the database). With `synchronous_commit = off`, PostgreSQL gives that same
  guarantee, and slonbase writes a little faster than PocketBase (1.10×; 0.68× with the
  default). Which one to use is a choice
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

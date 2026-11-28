# Benchmarks

How fast slonbase is, against PocketBase and PostgREST, on the same machine, the same data and the
same HTTP workloads. Everything here is repeatable: the scripts, the raw runs and the full
tables are in the repository's [`bench/`](https://github.com/ayeshrodz/slonbase/tree/main/bench).

## Results

<!-- bench:setup -->
slonbase 2.7.1, PocketBase 0.40.6 and PostgREST 16.5, each at its defaults (PostgREST with a pool of 16), PostgreSQL 18.6, on one machine (11th Gen Intel Core i5-1135G7 @ 2.40GHz, 8 CPUs, 15 GB), with the performance governor and turbo boost off: the servers and PostgreSQL on CPUs 0-2, the load generator on 3. 32 clients, medians of 5 runs of 10s per scenario, after a 2s warm-up.
<!-- bench:end -->

<!-- bench:summary -->
| Scenario | slonbase 2.7 req/s | PocketBase 0.40.6 req/s | PostgREST 16.5 req/s | p99 ms (slonbase / PocketBase / PostgREST) |
|---|--:|--:|--:|--:|
| Health check | 13,003 | 11,570 | 12,325 | 5.3 / 10.7 / 5.8 |
| View one record | 6,217 | 4,835 | 1,848 | 9.9 / 41.7 / 29.0 |
| List 30 records | 2,254 | 513 | 1,371 | 23.8 / 185 / 31.8 |
| List 30, filtered | 2,067 | 57.4 | 1,214 | 25.6 / 820 / 35.0 |
| List 30, expand a relation | 1,209 | 372 | n/a | 38.2 / 254 / n/a |
| Create a record | 2,575 | 2,457 | 1,436 | 18.5 / 58.7 / 29.6 |
<!-- bench:end -->

<!-- bench:claims -->
slonbase was ahead of PocketBase in 7 of 10 scenarios, up to 36× (filtered lists); level on creating records and sign-in, which bcrypt bounds; behind on single-record reads from one client (0.5×). Against PostgREST, it was ahead in every scenario measured, from 1.1× (the health check) to 3.4× (single-record reads).
<!-- bench:end -->

2.7.0 against 2.6.2 on the same workloads differs by less than the runs' own spread in every
scenario (creating records came out a little faster); the table and how it was measured are in
[`bench/README.md`](https://github.com/ayeshrodz/slonbase/blob/main/bench/README.md#what-changed-in-27).

## Footprint

<!-- bench:footprint -->
| | slonbase 2.7.1 | PocketBase 0.40.6 | PostgREST 16.5 |
|---|--:|--:|--:|
| Binary on disk | 26.1 MB | 32.0 MB | 19.6 MB |
| Resident memory, idle | 41 MB | 43 MB | 14 MB |
<!-- bench:end -->

<!-- bench:lightest -->
PostgREST is the leanest, and it does the least: no sign-in, files, realtime or dashboard. slonbase and PostgREST keep their data in a PostgreSQL server whose memory isn't counted; PocketBase's SQLite runs inside its process.
<!-- bench:end -->

## How to read them

- **Durability:** <!-- bench:durability -->PostgreSQL waits for each commit to reach disk by default; PocketBase's SQLite in WAL mode does not. The writes above are at PostgreSQL's default durability. `synchronous_commit = off` makes the same trade-off as SQLite and is faster (see [why](deployment.md#postgresql)).<!-- bench:end -->
- **Caveats:** one machine, loopback networking, and the database on the same CPUs as the
  server. A production setup with PostgreSQL on its own machine adds a network round trip to each
  slonbase query but gives the database cores of its own. Results on other hardware will differ;
  compare within a table, and repeat the runs yourself.
- **One client:** with a single client, each slonbase or PostgREST request waits for a round trip
  to PostgreSQL in another process, which PocketBase's SQLite, inside its own process, doesn't
  have. On a laptop, the idle cores' wake-up time adds to that round trip.
- **Not measured:** other platforms such as Supabase, Appwrite or Directus. No figures are given
  for anything that wasn't run.

The method, the per-release history and how to repeat the runs: [`bench/README.md`](https://github.com/ayeshrodz/slonbase/blob/main/bench/README.md).
See also [slonbase and PocketBase](slonbase-and-pocketbase.md) and [slonbase vs PostgREST](slonbase-vs-postgrest.md).

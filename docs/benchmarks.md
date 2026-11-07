# Benchmarks

How fast slonbase is, against PocketBase and PostgREST, on the same machine, the same data and the
same HTTP workloads. Everything here is repeatable: the scripts, the raw runs and the full
tables are in the repository's [`bench/`](https://github.com/ayeshrodz/slonbase/tree/main/bench).

## Results

slonbase 2.2.0, PocketBase 0.40.6 and PostgREST 16.5, each at its defaults (PostgREST with a pool
of 16), PostgreSQL 16, on a 4-vCPU cloud VM: the server and PostgreSQL pinned to 3 CPUs, the load
generator to the fourth. 32 clients, medians of three 10-second runs after a warm-up.

| Scenario | slonbase 2.2 req/s | PocketBase 0.40.6 req/s | PostgREST 16.5 req/s | p99 ms (slonbase / PocketBase / PostgREST) |
|---|--:|--:|--:|--:|
| Health check | 44,316 | 32,270 | 36,267 | 2.1 / 6.4 / 2.2 |
| View one record | 19,965 | 11,484 | 5,197 | 3.7 / 23.4 / 14.4 |
| List 30 records | 7,814 | 1,610 | 3,733 | 7.8 / 67.1 / 13.7 |
| List 30, filtered | 7,100 | 105 | 3,315 | 8.2 / 439 / 15.4 |
| List 30, expand a relation | 3,906 | 1,065 | n/a | 12.6 / 93.3 / n/a |
| Create a record | 7,424 | 4,051 | 3,828 | 8.4 / 45.5 / 16.2 |

slonbase was ahead in every scenario measured, from 1.2 times PocketBase's throughput on sign-in
(which bcrypt bounds) to 68 times on filtered lists, where PostgreSQL uses the index on the
filtered column.

## Footprint

| | slonbase 2.2.0 | PocketBase 0.40.6 | PostgREST 16.5 |
|---|--:|--:|--:|
| Binary on disk | 21.3 MB | 32.0 MB | 19.6 MB |
| Resident memory, idle | 40 MB | 41 MB | 15 MB |

PostgREST is the leanest, and it does the least: no sign-in, files, realtime or dashboard.
slonbase and PostgREST keep their data in a PostgreSQL server whose memory isn't counted;
PocketBase's SQLite runs inside its process.

## How to read them

- **Durability:** PostgreSQL waits for each commit to reach disk by default. PocketBase's SQLite
  in WAL mode does not, so with PostgreSQL's `synchronous_commit = off` (the same trade-off)
  slonbase creates 8,722 records a second.
- **Caveats:** one machine, loopback networking, and the database on the same CPUs as the
  server. A production setup with PostgreSQL on its own machine adds a network round trip to each
  slonbase query but gives the database cores of its own. Results on other hardware will differ;
  compare within a table, and repeat the runs yourself.
- **Not measured:** other platforms such as Supabase, Appwrite or Directus. No figures are given
  for anything that wasn't run.

The method, the per-release history and how to repeat the runs: [`bench/README.md`](https://github.com/ayeshrodz/slonbase/blob/main/bench/README.md).
See also [slonbase and PocketBase](slonbase-and-pocketbase.md) and [slonbase vs PostgREST](slonbase-vs-postgrest.md).

# Soak tests

Sustained, realistic traffic against slonbase, checking that nothing degrades over time.
Where the [benchmarks](../bench/) measure peak throughput against PocketBase, this measures
staying power: errors, memory, latency, connections and realtime, with several instances on one
database.

`run.sh` starts `INSTANCES` instances (2 by default) on one empty database, sharing their storage
directory, each with its metrics endpoint. `soak.mjs` then:

- creates an auth collection with a user per client, and a collection with rules, a relation, a
  file field and an index, seeded with 1,000 records;
- runs `SOAK_CLIENTS` clients (16) for `SOAK_DURATION` seconds (120), each sending its requests to
  a random instance, as a round-robin load balancer would: lists (sorted, filtered, with
  `expand`), views, creates, updates, deletes, file uploads and token refreshes;
- keeps `SOAK_SUBSCRIBERS` realtime subscribers (4) on the instances, subscribing through any
  instance (so subscriptions are forwarded);
- samples, every `SOAK_SAMPLE` seconds (10), the throughput, p50/p99 latency, errors, each
  instance's memory (RSS) and its database pool.

It fails (exit 1) when:

- any request fails, or an instance logs an error;
- a subscriber misses an event, or the p99 delay from a create to its event is over
  `SOAK_MAX_REALTIME_LAG_MS` (2,000);
- an instance's memory grows more than `SOAK_MAX_RSS_GROWTH_MB` (64) after the first quarter
  (the warm-up);
- the p99 latency at the end is more than `SOAK_MAX_P99_DRIFT` (3) times the one after warm-up;
- connections are still in use, or requests waiting for one, once the load stops.

```sh
cargo build --release
DATABASE_URL='postgres://...an empty database...' soak/run.sh
DATABASE_URL=... INSTANCES=3 SOAK_DURATION=3600 SOAK_CLIENTS=32 soak/run.sh   # an hour, 3 instances
```

The summary, with every sample, is written to `soak/.run/result.json`, and the instances' logs
to `soak/.run/server-*.log`. CI runs it weekly for 20 minutes on two instances (the `Soak`
workflow, which can also be started by hand with another duration).

The first runs found that realtime delivery fell behind under sustained writes (events took
seconds to arrive, and the delay kept growing): events were checked and sent one at a time. They
are now read in batches and checked several at a time, still in order for each client.

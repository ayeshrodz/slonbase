# Several instances on one PostgreSQL

Starts N slonbase instances (default 3) behind a small round-robin balancer with no affinity
(`lb.mjs`), all on one PostgreSQL database, and runs `scenarios.mjs` through the balancer:

1. tokens: a superuser and a user token work on every instance; changing a password, deleting a
   user and `revoke-tokens` end old tokens everywhere at once;
2. schema and settings changes reach the other instances;
3. a rate limit is shared (30 allowed means 30 across all instances);
4. realtime: events from any instance, and from plain SQL, reach a stream held by one instance;
   subscribe requests that land elsewhere are forwarded;
5. projects: created through one instance, served by all, collection changes propagate;
6. Health: `sync` and `connections` checks are green; the script ends by printing the number of
   PostgreSQL connections in use.

```sh
cargo build
FLEET_ADMIN_URL='postgres://postgres:postgres@127.0.0.1:5432/postgres?sslmode=disable' \
  scripts/fleet-test/run.sh          # 3 instances
scripts/fleet-test/run.sh 6          # 6 instances
FLEET_POOLER=pgbouncer scripts/fleet-test/run.sh   # through PgBouncer (transaction mode)
```

Needs Node 22, `psql`, a PostgreSQL role that can create roles and databases, and `pgbouncer`
for the pooled mode. With `FLEET_POOLER=pgbouncer`, queries go through PgBouncer and the
listeners connect directly (`--database-listen-url`). The exit code is the number of failed
checks. Set `SLONBASE_BIN` to test a release binary.

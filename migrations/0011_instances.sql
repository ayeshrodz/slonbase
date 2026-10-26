-- Instances sharing the database (a heartbeat every 20 seconds), realtime subscribe
-- requests forwarded to the instance that holds the client's event stream, so realtime
-- works behind a load balancer without client affinity, and shared rate limits.
CREATE TABLE _slonbase.instances (
    id   TEXT PRIMARY KEY,
    seen TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE _slonbase.realtime_forwards (
    id        BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client_id TEXT NOT NULL,
    request   JSONB NOT NULL,
    response  JSONB,
    created   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Rate limit windows shared by the instances (used when more than one is live; a single
-- instance counts in memory). Unlogged: counters needn't survive a crash.
CREATE UNLOGGED TABLE _slonbase.rate_limits (
    key       TEXT PRIMARY KEY,
    started   TIMESTAMPTZ NOT NULL,
    remaining INTEGER NOT NULL
);

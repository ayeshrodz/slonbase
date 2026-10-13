-- Request and app logs. UNLOGGED: no write-ahead log, so writing them is cheap; they're
-- emptied after a database crash and aren't replicated, which is fine for logs.
CREATE UNLOGGED TABLE _slonbase.logs (
    id      TEXT PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    level   INTEGER NOT NULL,
    message TEXT NOT NULL,
    data    JSONB NOT NULL DEFAULT '{}',
    -- insertion order, for sorting by `@rowid` like record tables
    _seq    BIGINT GENERATED ALWAYS AS IDENTITY
);

CREATE INDEX logs_created_idx ON _slonbase.logs (created);

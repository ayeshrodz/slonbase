-- Outbox of record changes. Rows are written in the same transaction as the change
-- and announced with NOTIFY, so every instance can deliver realtime events (and a
-- rolled-back change is never announced). Old rows are pruned regularly.
CREATE TABLE _slonbase.events (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    collection_id TEXT NOT NULL,
    action        TEXT NOT NULL,
    record        JSONB NOT NULL,
    created       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX events_created_idx ON _slonbase.events (created);

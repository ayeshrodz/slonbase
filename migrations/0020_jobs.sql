-- Work to do in the background: each row is one job, claimed by one instance at a time
-- (FOR UPDATE SKIP LOCKED) and held with a lease, so a job outlives the instance that queued
-- it and several instances share the work. `dedupe`, when set, makes a job exist once (a
-- scheduled job, once a minute, whichever instance notices first). Finished rows are kept a
-- week.
CREATE TABLE IF NOT EXISTS _slonbase.jobs (
    id BIGSERIAL PRIMARY KEY,
    kind TEXT NOT NULL,
    ref TEXT NOT NULL DEFAULT '',
    payload JSONB NOT NULL DEFAULT '{}',
    state TEXT NOT NULL DEFAULT 'queued',
    attempt INT NOT NULL DEFAULT 0,
    max_attempts INT NOT NULL DEFAULT 1,
    run_after TIMESTAMPTZ NOT NULL DEFAULT now(),
    dedupe TEXT UNIQUE,
    locked_by TEXT,
    locked_until TIMESTAMPTZ,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    started TIMESTAMPTZ,
    finished TIMESTAMPTZ,
    error TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS jobs_pending ON _slonbase.jobs (run_after)
    WHERE state IN ('queued', 'running');

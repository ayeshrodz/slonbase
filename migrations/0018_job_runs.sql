-- What scheduled and manual jobs did: automatic and manual backups, and cron jobs. The last
-- 500 runs of each job are kept.
CREATE TABLE IF NOT EXISTS _slonbase.job_runs (
    id BIGSERIAL PRIMARY KEY,
    kind TEXT NOT NULL,
    job TEXT NOT NULL,
    trigger TEXT NOT NULL,
    started TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished TIMESTAMPTZ,
    status TEXT NOT NULL DEFAULT 'running',
    detail TEXT NOT NULL DEFAULT '',
    meta JSONB NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS job_runs_job ON _slonbase.job_runs (kind, job, started DESC);

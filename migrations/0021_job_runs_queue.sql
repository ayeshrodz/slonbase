-- A run in the jobs' history knows the queued job it belongs to, so a job that is tried again
-- (after a failure, or after its instance stopped) continues the same run instead of starting
-- a new one.
ALTER TABLE _slonbase.job_runs ADD COLUMN IF NOT EXISTS queue_job BIGINT;
CREATE INDEX IF NOT EXISTS job_runs_queue_job ON _slonbase.job_runs (queue_job)
    WHERE queue_job IS NOT NULL;

-- Scheduled jobs are queued once a minute by the queue's own key: the claims kept in the
-- params table have no use any more.
DELETE FROM _slonbase.params WHERE key LIKE 'cron\_last:%' OR key = 'backups_last_auto';

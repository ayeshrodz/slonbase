-- Where a run of an automation stands while it goes on: the next step, and what the steps
-- before it produced. An attempt that follows a failed one continues from there instead of
-- doing the finished steps (an email, a write) again. Cleared when the run ends.
ALTER TABLE _slonbase.job_runs ADD COLUMN IF NOT EXISTS state JSONB;

-- What each instance reports about itself with its heartbeat (host, version, uptime, memory, pool
-- and whether it hears the others), so Health can show every instance sharing the database.
-- Instances of an older version leave it empty.
ALTER TABLE _slonbase.instances ADD COLUMN info JSONB NOT NULL DEFAULT '{}'::jsonb;

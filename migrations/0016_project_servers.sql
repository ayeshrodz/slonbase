-- Projects whose database is on another PostgreSQL server (or one slonbase didn't create): its
-- connection URL, sealed with the settings key when there is one. NULL for a database slonbase
-- created on the main installation's server.
ALTER TABLE _slonbase.projects ADD COLUMN url TEXT;
-- database names are unique on the main server only
ALTER TABLE _slonbase.projects DROP CONSTRAINT IF EXISTS projects_database_key;
CREATE UNIQUE INDEX IF NOT EXISTS projects_local_database ON _slonbase.projects (database) WHERE url IS NULL;

-- Projects with a PostgreSQL role of their own: the role owns the project's database and is
-- the only one (besides the main installation's) that can connect to it.
ALTER TABLE _slonbase.projects
    ADD COLUMN role TEXT,
    ADD COLUMN role_password TEXT;

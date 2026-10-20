-- The JavaScript migrations (PocketBase's pb_migrations) that have been applied, by file name.
CREATE TABLE _slonbase.app_migrations (
    file TEXT PRIMARY KEY,
    applied TIMESTAMPTZ NOT NULL DEFAULT now()
);

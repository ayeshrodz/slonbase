-- Branding assets: logos, icons and images the dashboard, its sign-in page and emails show.
-- Small (at most 2 MB each), so they live in the database and go with its backups.
CREATE TABLE IF NOT EXISTS _slonbase.assets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    mime TEXT NOT NULL,
    size INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    data BYTEA NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now()
);

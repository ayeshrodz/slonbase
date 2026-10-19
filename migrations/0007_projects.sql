-- Projects: further installations hosted by this one, each in its own database.
-- Only the main installation's table is used.
CREATE TABLE _slonbase.projects (
    name     TEXT PRIMARY KEY,
    database TEXT NOT NULL UNIQUE,
    -- host names that route to the project, besides /p/{name}/
    hosts    TEXT[] NOT NULL DEFAULT '{}',
    created  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- System tables live in the "_slonbase" schema so slonbase can share
-- a database with other applications. User collections will get their own schema.

CREATE TABLE _slonbase.superusers (
    id            TEXT PRIMARY KEY,
    email         TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    -- rotating this invalidates all issued auth tokens for the superuser
    token_key     TEXT NOT NULL,
    created       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX superusers_email_idx ON _slonbase.superusers (lower(email));

-- Key/value store for app settings and internal state.
CREATE TABLE _slonbase.params (
    key     TEXT PRIMARY KEY,
    value   JSONB NOT NULL,
    updated TIMESTAMPTZ NOT NULL DEFAULT now()
);

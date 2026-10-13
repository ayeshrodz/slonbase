-- Collection definitions (fields, indexes and rules in PocketBase's JSON format).
CREATE TABLE _slonbase.collections (
    id      TEXT PRIMARY KEY,
    name    TEXT NOT NULL,
    data    JSONB NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX collections_name_idx ON _slonbase.collections (lower(name));

-- Collection tables live in their own schema.
CREATE SCHEMA slonbase;

-- Converts text to a number, NULL when it isn't one (used to compare JSON values
-- with numbers in filters without failing on non-numeric values).
CREATE FUNCTION _slonbase.to_num(v TEXT) RETURNS DOUBLE PRECISION
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN v ~ '^\s*[-+]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][-+]?[0-9]+)?\s*$'
        THEN v::double precision
    END
$$;

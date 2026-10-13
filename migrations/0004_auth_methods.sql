-- Sign-in methods beyond passwords. Rows of a deleted collection go with it; rows of a
-- deleted record are removed with the record.

-- Accounts of OAuth2 providers linked to auth records.
CREATE TABLE _slonbase.external_auths (
    id            TEXT PRIMARY KEY,
    collection_id TEXT NOT NULL REFERENCES _slonbase.collections (id) ON DELETE CASCADE,
    record_id     TEXT NOT NULL,
    provider      TEXT NOT NULL,
    provider_id   TEXT NOT NULL,
    created       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (collection_id, provider, provider_id),
    UNIQUE (collection_id, record_id, provider)
);

-- One-time passwords sent by email (only their hashes are stored).
CREATE TABLE _slonbase.otps (
    id            TEXT PRIMARY KEY,
    collection_id TEXT NOT NULL REFERENCES _slonbase.collections (id) ON DELETE CASCADE,
    record_id     TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    sent_to       TEXT NOT NULL DEFAULT '',
    attempts      INTEGER NOT NULL DEFAULT 0,
    created       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX otps_record_idx ON _slonbase.otps (collection_id, record_id);
CREATE INDEX otps_created_idx ON _slonbase.otps (created);

-- Sign-ins waiting for their second method (multi-factor authentication).
CREATE TABLE _slonbase.mfas (
    id            TEXT PRIMARY KEY,
    collection_id TEXT NOT NULL REFERENCES _slonbase.collections (id) ON DELETE CASCADE,
    record_id     TEXT NOT NULL,
    method        TEXT NOT NULL,
    created       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX mfas_record_idx ON _slonbase.mfas (collection_id, record_id);
CREATE INDEX mfas_created_idx ON _slonbase.mfas (created);

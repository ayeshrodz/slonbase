-- Devices (IP address and user agent) records have signed in from, for new-device alerts.
-- Each row carries a hash of the record's token key at the time, so a password change
-- (which rotates the key) forgets the record's known devices.
CREATE TABLE _slonbase.auth_origins (
    collection_id TEXT NOT NULL REFERENCES _slonbase.collections (id) ON DELETE CASCADE,
    record_id     TEXT NOT NULL,
    fingerprint   TEXT NOT NULL,
    key_hash      TEXT NOT NULL,
    created       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (collection_id, record_id, fingerprint)
);

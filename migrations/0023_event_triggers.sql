-- Automations that run when a record changes read the same outbox realtime does
-- (`_slonbase.events`), from a place of their own (`event_cursor`): the snapshot up to which
-- changes were turned into runs, moved in the transaction that queues those runs, so each
-- change starts its automations once whichever instance looks, and none is lost in a restart.
--
-- For that an event also says which fields an update changed (`changed`, only for the
-- collections an automation watches: a lookup in `event_watchers` for an update and nothing
-- else for any other write) and what made the change when an automation did (`origin`: the
-- automations a chain of changes went through, so none runs twice in a chain).
--
-- `event_watchers` has each automation that watches, with the moment it began to (a change
-- older than that is not its own). The cursor also holds what the watchers are (`watching`, so
-- a dispatcher that has not heard of a change to them stands back) and when automations were
-- paused (`pauses`: `[{"from", "to"}]` for the last day, `to` missing while paused; changes
-- made then are not run, however late they commit). All of it is written with the settings,
-- in their transaction.
ALTER TABLE _slonbase.events ADD COLUMN IF NOT EXISTS changed TEXT[];
ALTER TABLE _slonbase.events ADD COLUMN IF NOT EXISTS origin TEXT;

CREATE TABLE IF NOT EXISTS _slonbase.event_watchers (
    job           TEXT PRIMARY KEY,
    collection_id TEXT NOT NULL,
    since         TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX IF NOT EXISTS event_watchers_collection ON _slonbase.event_watchers (collection_id);

CREATE TABLE IF NOT EXISTS _slonbase.event_cursor (
    one         BOOLEAN PRIMARY KEY DEFAULT true CHECK (one),
    snapshot    TEXT NOT NULL,
    watching    TEXT NOT NULL DEFAULT '',
    pauses      JSONB NOT NULL DEFAULT '[]',
    updated     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE OR REPLACE FUNCTION _slonbase.record_change() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    record jsonb;
    col text;
    changed text[];
BEGIN
    IF current_setting('slonbase.realtime', true) = 'off' THEN
        RETURN NULL;
    END IF;
    IF TG_OP = 'DELETE' THEN
        record := to_jsonb(OLD);
    ELSE
        record := to_jsonb(NEW);
    END IF;
    IF TG_OP = 'UPDATE' AND EXISTS (
        SELECT 1 FROM _slonbase.event_watchers w WHERE w.collection_id = TG_ARGV[0]
    ) THEN
        SELECT coalesce(array_agg(n.key ORDER BY n.key), '{}') INTO changed
        FROM jsonb_each(record) n
        WHERE n.key <> '_seq' AND n.key <> ALL (TG_ARGV[1]::text[])
          AND n.value IS DISTINCT FROM to_jsonb(OLD) -> n.key;
    END IF;
    record := record - '_seq' - TG_ARGV[1]::text[];
    FOREACH col IN ARRAY TG_ARGV[2]::text[] LOOP
        record := jsonb_set(record, ARRAY[col], to_jsonb(coalesce(
            to_char((record->>col)::timestamptz AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.MS"Z"'),
            ''
        )));
    END LOOP;
    INSERT INTO _slonbase.events (collection_id, action, record, changed, origin)
    VALUES (
        TG_ARGV[0],
        CASE TG_OP WHEN 'INSERT' THEN 'create' WHEN 'UPDATE' THEN 'update' ELSE 'delete' END,
        record,
        changed,
        nullif(current_setting('slonbase.origin', true), '')
    );
    IF current_setting('slonbase.notify', true) IS DISTINCT FROM 'off' THEN
        PERFORM pg_notify('slonbase', 'changed');
    END IF;
    RETURN NULL;
END
$$;

-- The record trigger of 0023 looked up who watches a collection for every write, though only
-- an update needs it (to name the changed fields): PostgreSQL evaluates the whole condition
-- `TG_OP = 'UPDATE' AND EXISTS (...)` rather than stopping at the first part, which made an
-- insert about a fifth slower in the database. The same function, with the lookup only for
-- updates.
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
    -- (nested: in one condition the lookup would run for every write, not only updates)
    IF TG_OP = 'UPDATE' THEN
        IF EXISTS (
            SELECT 1 FROM _slonbase.event_watchers w WHERE w.collection_id = TG_ARGV[0]
        ) THEN
            SELECT coalesce(array_agg(n.key ORDER BY n.key), '{}') INTO changed
            FROM jsonb_each(record) n
            WHERE n.key <> '_seq' AND n.key <> ALL (TG_ARGV[1]::text[])
              AND n.value IS DISTINCT FROM to_jsonb(OLD) -> n.key;
        END IF;
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

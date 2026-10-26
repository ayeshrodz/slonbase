-- Realtime events for every change of a collection table, whatever makes it (the records API,
-- SQL through hooks or the SQL console, other programs): the outbox row and its NOTIFY, in the
-- change's transaction, as the API wrote them before. Arguments: the collection id, the
-- columns left out (secrets) and the date columns (sent formatted as the API sends them).
-- `SET slonbase.realtime = off` skips them, for bulk loads (restores, imports).
CREATE FUNCTION _slonbase.record_change() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    record jsonb;
    col text;
    event_id bigint;
BEGIN
    IF current_setting('slonbase.realtime', true) = 'off' THEN
        RETURN NULL;
    END IF;
    IF TG_OP = 'DELETE' THEN
        record := to_jsonb(OLD);
    ELSE
        record := to_jsonb(NEW);
    END IF;
    record := record - '_seq' - TG_ARGV[1]::text[];
    FOREACH col IN ARRAY TG_ARGV[2]::text[] LOOP
        record := jsonb_set(record, ARRAY[col], to_jsonb(coalesce(
            to_char((record->>col)::timestamptz AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS.MS"Z"'),
            ''
        )));
    END LOOP;
    INSERT INTO _slonbase.events (collection_id, action, record)
    VALUES (
        TG_ARGV[0],
        CASE TG_OP WHEN 'INSERT' THEN 'create' WHEN 'UPDATE' THEN 'update' ELSE 'delete' END,
        record
    )
    RETURNING id INTO event_id;
    PERFORM pg_notify('slonbase', 'event:' || event_id);
    RETURN NULL;
END
$$;

-- the trigger on the existing tables (new and changed collections get it with their DDL)
DO $$
DECLARE
    c record;
BEGIN
    FOR c IN
        SELECT id, name, data FROM _slonbase.collections WHERE coalesce(data->>'type', 'base') <> 'view'
    LOOP
        EXECUTE format(
            'CREATE OR REPLACE TRIGGER _slonbase_realtime AFTER INSERT OR UPDATE OR DELETE ON slonbase.%I '
            'FOR EACH ROW EXECUTE FUNCTION _slonbase.record_change(%L, %L, %L)',
            c.name,
            c.id,
            coalesce((
                SELECT array_agg(f->>'name' ORDER BY f->>'name')
                FROM jsonb_array_elements(c.data->'fields') f
                WHERE f->>'type' = 'password' OR (f->>'name' = 'tokenKey' AND (f->>'system')::boolean)
            ), '{}'::text[])::text,
            coalesce((
                SELECT array_agg(f->>'name' ORDER BY f->>'name')
                FROM jsonb_array_elements(c.data->'fields') f
                WHERE f->>'type' IN ('date', 'autodate')
            ), '{}'::text[])::text
        );
    END LOOP;
END
$$;

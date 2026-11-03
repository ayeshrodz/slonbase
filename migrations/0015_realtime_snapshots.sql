-- Events are read by transaction snapshot rather than announced one by one: a NOTIFY in a
-- writing transaction takes a database-wide lock until its commit is flushed, so every write
-- that notified waited for the one before it (hundreds of commits a second at most). Now each
-- event records its transaction, and every instance reads the events committed since its last
-- snapshot (visible now, not then): each event once, none missed, in commit order.
--
-- slonbase's own connections set `slonbase.notify = off` and announce their writes after they
-- commit, once for many; other programs' writes still notify from the trigger (`changed`).
ALTER TABLE _slonbase.events ADD COLUMN xid xid8 NOT NULL DEFAULT pg_current_xact_id();
CREATE INDEX events_xid_idx ON _slonbase.events (xid);

CREATE OR REPLACE FUNCTION _slonbase.record_change() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    record jsonb;
    col text;
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
    );
    IF current_setting('slonbase.notify', true) IS DISTINCT FROM 'off' THEN
        PERFORM pg_notify('slonbase', 'changed');
    END IF;
    RETURN NULL;
END
$$;

-- strftime() in filters: SQLite's strftime(format, time-value, modifiers...), which PocketBase's
-- filters expose, with SQLite's time values, modifiers and format codes. It follows SQLite's
-- date.c closely: a time is a Julian day in milliseconds (`jd`) and/or its fields, so month
-- overflow, rounding and range checks give SQLite's results. `localtime` and `utc` treat the
-- server's time zone as UTC. `now_ms` is the request's time in Unix milliseconds ('now'). Returns
-- NULL where SQLite does (the filter compiler turns that into '').

CREATE TYPE _slonbase.sqlite_time AS (
    jd bigint,
    valid_jd boolean,
    y int,
    m int,
    d int,
    valid_ymd boolean,
    h int,
    mi int,
    s double precision,
    valid_hms boolean,
    tz int,
    raw_s boolean,
    raw double precision,
    is_utc boolean,
    is_local boolean,
    n_floor int,
    subsec boolean,
    err boolean
);

-- datetimeError: a cleared time, marked as an error (later modifiers still apply to it, and
-- `localtime` and `utc` clear the mark, as in SQLite)
CREATE FUNCTION _slonbase.sqlite_error() RETURNS _slonbase.sqlite_time
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT ROW(0, false, 0, 0, 0, false, 0, 0, 0, false, 0, false, 0, false, false, 0, false, true)::_slonbase.sqlite_time
$$;

-- the Julian day (in ms) of a date, NULL out of range (computeJD's date part)
CREATE FUNCTION _slonbase.sqlite_jd(y int, m int, d int) RETURNS bigint
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
    a int;
    b int;
    x1 int;
    x2 int;
BEGIN
    IF y < -4713 OR y > 9999 THEN
        RETURN NULL;
    END IF;
    IF m <= 2 THEN
        y := y - 1;
        m := m + 12;
    END IF;
    a := (y + 4800) / 100;
    b := 38 - a + (a / 4);
    x1 := 36525 * (y + 4716) / 100;
    x2 := 306001 * (m + 1) / 10000;
    RETURN trunc((x1 + x2 + d + b - 1524.5::double precision) * 86400000)::bigint;
END
$$;

-- computeJD
CREATE FUNCTION _slonbase.sqlite_compute_jd(p _slonbase.sqlite_time) RETURNS _slonbase.sqlite_time
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
BEGIN
    IF p.valid_jd THEN
        RETURN p;
    END IF;
    IF p.raw_s THEN
        RETURN _slonbase.sqlite_error();
    END IF;
    IF p.valid_ymd THEN
        p.jd := _slonbase.sqlite_jd(p.y, p.m, p.d);
    ELSE
        p.jd := _slonbase.sqlite_jd(2000, 1, 1);
    END IF;
    IF p.jd IS NULL THEN
        RETURN _slonbase.sqlite_error();
    END IF;
    p.valid_jd := true;
    IF p.valid_hms THEN
        p.jd := p.jd + p.h * 3600000::bigint + p.mi * 60000::bigint + trunc(p.s * 1000 + 0.5)::bigint;
        IF p.tz <> 0 THEN
            p.jd := p.jd - p.tz * 60000::bigint;
            p.valid_ymd := false;
            p.valid_hms := false;
            p.tz := 0;
            p.is_utc := true;
            p.is_local := false;
        END IF;
    END IF;
    RETURN p;
END
$$;

-- computeYMD
CREATE FUNCTION _slonbase.sqlite_compute_ymd(p _slonbase.sqlite_time) RETURNS _slonbase.sqlite_time
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
    z int;
    a int;
    b int;
    c int;
    d int;
    e int;
    x1 int;
BEGIN
    IF p.valid_ymd THEN
        RETURN p;
    END IF;
    IF NOT p.valid_jd THEN
        p.y := 2000;
        p.m := 1;
        p.d := 1;
    ELSIF p.jd < 0 OR p.jd > 464269060799999 THEN
        RETURN _slonbase.sqlite_error();
    ELSE
        z := ((p.jd + 43200000) / 86400000)::int;
        a := trunc((z - 1867216.25::double precision) / 36524.25)::int;
        a := z + 1 + a - (a / 4);
        b := a + 1524;
        c := trunc((b - 122.1::double precision) / 365.25)::int;
        d := (36525 * (c & 32767)) / 100;
        e := trunc((b - d) / 30.6001::double precision)::int;
        x1 := trunc(30.6001::double precision * e)::int;
        p.d := b - d - x1;
        p.m := CASE WHEN e < 14 THEN e - 1 ELSE e - 13 END;
        p.y := CASE WHEN p.m > 2 THEN c - 4716 ELSE c - 4715 END;
    END IF;
    p.valid_ymd := true;
    RETURN p;
END
$$;

-- computeHMS
CREATE FUNCTION _slonbase.sqlite_compute_hms(p _slonbase.sqlite_time) RETURNS _slonbase.sqlite_time
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
    day_ms int;
BEGIN
    IF p.valid_hms THEN
        RETURN p;
    END IF;
    p := _slonbase.sqlite_compute_jd(p);
    day_ms := ((p.jd + 43200000) % 86400000)::int;
    p.s := (day_ms % 60000) / 1000.0::double precision;
    p.mi := (day_ms / 60000) % 60;
    p.h := (day_ms / 60000) / 60;
    p.raw_s := false;
    p.valid_hms := true;
    RETURN p;
END
$$;

-- computeFloor: how many days a YYYY-MM-DD overflows its month by
CREATE FUNCTION _slonbase.sqlite_floor(p _slonbase.sqlite_time) RETURNS int
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN p.d <= 28 THEN 0
        WHEN ((1 << p.m) & 5546) <> 0 THEN 0
        WHEN p.m <> 2 THEN (p.d = 31)::int
        WHEN p.y % 4 <> 0 OR (p.y % 100 = 0 AND p.y % 400 <> 0) THEN p.d - 28
        ELSE p.d - 29
    END
$$;

-- clearYMD_HMS_TZ
CREATE FUNCTION _slonbase.sqlite_clear(p _slonbase.sqlite_time) RETURNS _slonbase.sqlite_time
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
BEGIN
    p.valid_ymd := false;
    p.valid_hms := false;
    p.tz := 0;
    RETURN p;
END
$$;

-- parseHhMmSs: HH:MM[:SS[.FFF]] and a time zone, into p (NULL if it isn't one)
CREATE FUNCTION _slonbase.sqlite_parse_hms(z text, p _slonbase.sqlite_time) RETURNS _slonbase.sqlite_time
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
    t text[];
    zone text[];
    ms double precision := 0;
BEGIN
    t := regexp_match(z, '^([0-9]{2}):([0-9]{2})(?::([0-9]{2})(?:\.([0-9]+))?)?(.*)$');
    IF t IS NULL OR t[1]::int > 24 OR t[2]::int > 59 OR coalesce(t[3]::int, 0) > 59 THEN
        RETURN NULL;
    END IF;
    IF t[4] IS NOT NULL THEN
        ms := least(('0.' || t[4])::double precision, 0.999);
    END IF;
    -- parseTimezone
    zone := regexp_match(t[5], '^[[:space:]]*(?:([Zz])|([+-])([0-9]{2}):([0-9]{2}))?[[:space:]]*$');
    IF zone IS NULL OR (zone[2] IS NOT NULL AND (zone[3]::int > 14 OR zone[4]::int > 59)) THEN
        RETURN NULL;
    END IF;
    p.tz := 0;
    IF zone[1] IS NOT NULL THEN
        p.is_local := false;
        p.is_utc := true;
    ELSIF zone[2] IS NOT NULL THEN
        p.tz := (CASE zone[2] WHEN '-' THEN -1 ELSE 1 END) * (zone[4]::int + zone[3]::int * 60);
    END IF;
    p.valid_jd := false;
    p.raw_s := false;
    p.valid_hms := true;
    p.h := t[1]::int;
    p.mi := t[2]::int;
    p.s := coalesce(t[3]::int, 0) + ms;
    RETURN p;
END
$$;

-- sqlite3AtoF: the whole text is a number
CREATE FUNCTION _slonbase.sqlite_number(z text) RETURNS double precision
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN z ~ '^[[:space:]]*[+-]?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][+-]?[0-9]+)?[[:space:]]*$'
        THEN btrim(z, E' \t\n\r\f\v')::double precision
    END
$$;

-- parseDateOrTime
CREATE FUNCTION _slonbase.sqlite_parse_time(z text, now_ms bigint) RETURNS _slonbase.sqlite_time
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
    p _slonbase.sqlite_time := ROW(0, false, 0, 0, 0, false, 0, 0, 0, false, 0, false, 0, false, false, 0, false, false);
    q _slonbase.sqlite_time;
    t text[];
    r double precision;
BEGIN
    -- parseYyyyMmDd
    t := regexp_match(z, '^(-?)([0-9]{4})-([0-9]{2})-([0-9]{2})[[:space:]T]*(.*)$');
    IF t IS NOT NULL THEN
        IF t[3]::int NOT BETWEEN 1 AND 12 OR t[4]::int NOT BETWEEN 1 AND 31 THEN
            RETURN NULL;
        END IF;
        IF t[5] <> '' THEN
            p := _slonbase.sqlite_parse_hms(t[5], p);
            IF p IS NULL THEN
                RETURN NULL;
            END IF;
        END IF;
        p.valid_jd := false;
        p.valid_ymd := true;
        p.y := CASE WHEN t[1] = '-' THEN -t[2]::int ELSE t[2]::int END;
        p.m := t[3]::int;
        p.d := t[4]::int;
        p.n_floor := _slonbase.sqlite_floor(p);
        IF p.tz <> 0 THEN
            p := _slonbase.sqlite_compute_jd(p);
        END IF;
        RETURN p;
    END IF;
    q := _slonbase.sqlite_parse_hms(z, p);
    IF q IS NOT NULL THEN
        RETURN q;
    END IF;
    IF lower(z) = 'now' THEN
        p.jd := now_ms + 210866760000000;
        p.valid_jd := true;
        RETURN p;
    END IF;
    r := _slonbase.sqlite_number(z);
    IF r IS NOT NULL THEN
        p.raw := r;
        p.raw_s := true;
        IF r >= 0 AND r < 5373484.5 THEN
            p.jd := trunc(r * 86400000 + 0.5)::bigint;
            p.valid_jd := true;
        END IF;
        RETURN p;
    END IF;
    IF lower(z) IN ('subsec', 'subsecond') THEN
        p.subsec := true;
        p.jd := now_ms + 210866760000000;
        p.valid_jd := true;
        RETURN p;
    END IF;
    RETURN NULL;
END
$$;

-- toLocaltime, with the server's time zone as UTC (the fields of the same instant, through the
-- same year mapping SQLite uses outside 1800-2600)
CREATE FUNCTION _slonbase.sqlite_localtime(p _slonbase.sqlite_time) RETURNS _slonbase.sqlite_time
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
    x _slonbase.sqlite_time;
    diff int := 0;
    t bigint;
BEGIN
    p := _slonbase.sqlite_compute_jd(p);
    IF p.jd < 210866760000000 OR p.jd > 213014145600000 THEN
        x := _slonbase.sqlite_compute_hms(_slonbase.sqlite_compute_ymd(p));
        diff := (2000 + x.y % 4) - x.y;
        x.y := x.y + diff;
        x.valid_jd := false;
        x := _slonbase.sqlite_compute_jd(x);
        t := x.jd / 1000;
    ELSE
        t := p.jd / 1000;
    END IF;
    x := _slonbase.sqlite_compute_hms(_slonbase.sqlite_compute_ymd(
        ROW(t * 1000, true, 0, 0, 0, false, 0, 0, 0, false, 0, false, 0, false, false, 0, false, false)));
    p.y := x.y - diff;
    p.m := x.m;
    p.d := x.d;
    p.h := x.h;
    p.mi := x.mi;
    p.s := trunc(x.s) + (p.jd % 1000) * 0.001;
    p.valid_ymd := true;
    p.valid_hms := true;
    p.valid_jd := false;
    p.raw_s := false;
    p.tz := 0;
    p.err := false;
    p.is_utc := false;
    p.is_local := true;
    RETURN p;
END
$$;

-- the exact value of a double
CREATE FUNCTION _slonbase.sqlite_exact(v double precision) RETURNS numeric
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
    bits bigint := ('x' || encode(float8send(v), 'hex'))::bit(64)::bigint;
    e int := ((bits >> 52) & 2047)::int;
    m numeric := (bits & 4503599627370495)::numeric;
    r numeric;
BEGIN
    IF e = 0 THEN
        e := 1;
    ELSE
        m := m + 4503599627370496;
    END IF;
    e := e - 1075;
    IF e >= 0 THEN
        r := m * (2::numeric ^ e);
    ELSE
        r := m;
        FOR i IN 1 .. -e LOOP
            r := r * 5;
        END LOOP;
        r := r * ('1e' || e)::numeric;
    END IF;
    RETURN CASE WHEN bits < 0 THEN -r ELSE r END;
END
$$;

-- SQLite's printf of a double: %.{n}f, or %.{n}g when significant
CREATE FUNCTION _slonbase.sqlite_printf(v double precision, n int, significant boolean) RETURNS text
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
    x numeric := _slonbase.sqlite_exact(v);
    digits int;
BEGIN
    IF NOT significant THEN
        RETURN round(x, n)::text;
    END IF;
    IF x = 0 THEN
        RETURN '0';
    END IF;
    digits := floor(log(abs(x)))::int + 1;
    x := round(x, n - digits);
    RETURN trim_scale(x)::text;
END
$$;

-- parseModifier; NULL when the modifier is invalid
CREATE FUNCTION _slonbase.sqlite_modify(p _slonbase.sqlite_time, z text, idx int) RETURNS _slonbase.sqlite_time
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
    lz text := lower(z);
    r double precision;
    n int;
    t text[];
    tx _slonbase.sqlite_time;
    tx_ms bigint;
    x int;
    unit text;
    limit_ double precision;
    xform double precision;
    rounder double precision;
    z2 text;
BEGIN
    IF lz = 'auto' THEN
        IF idx > 1 THEN
            RETURN NULL;
        END IF;
        IF NOT p.raw_s OR p.valid_jd THEN
            p.raw_s := false;
            RETURN p;
        ELSIF p.raw >= -210866760000 AND p.raw <= 253402300799 THEN
            p := _slonbase.sqlite_clear(p);
            p.jd := trunc(p.raw * 1000 + 210866760000000 + 0.5)::bigint;
            p.valid_jd := true;
            p.raw_s := false;
            RETURN p;
        END IF;
        RETURN NULL;
    ELSIF lz = 'ceiling' THEN
        p := _slonbase.sqlite_clear(_slonbase.sqlite_compute_jd(p));
        p.n_floor := 0;
        RETURN p;
    ELSIF lz = 'floor' THEN
        p := _slonbase.sqlite_compute_jd(p);
        p.jd := p.jd - p.n_floor * 86400000::bigint;
        RETURN _slonbase.sqlite_clear(p);
    ELSIF lz = 'julianday' THEN
        IF idx > 1 THEN
            RETURN NULL;
        END IF;
        IF p.valid_jd AND p.raw_s THEN
            p.raw_s := false;
            RETURN p;
        END IF;
        RETURN NULL;
    ELSIF lz = 'localtime' THEN
        -- the server's time zone is UTC: the same instant, as fields
        IF NOT p.is_local THEN
            p := _slonbase.sqlite_localtime(p);
        END IF;
        p.is_utc := false;
        p.is_local := true;
        RETURN p;
    ELSIF lz = 'unixepoch' AND p.raw_s THEN
        IF idx > 1 THEN
            RETURN NULL;
        END IF;
        r := p.raw * 1000 + 210866760000000;
        IF r >= 0 AND r < 464269060800000 THEN
            p := _slonbase.sqlite_clear(p);
            p.jd := trunc(r + 0.5)::bigint;
            p.valid_jd := true;
            p.raw_s := false;
            RETURN p;
        END IF;
        RETURN NULL;
    ELSIF lz = 'utc' THEN
        IF NOT p.is_utc THEN
            -- the instant the local time stands for (the same one, in UTC)
            p := _slonbase.sqlite_compute_jd(p);
            tx := _slonbase.sqlite_compute_jd(_slonbase.sqlite_localtime(
                ROW(p.jd, true, 0, 0, 0, false, 0, 0, 0, false, 0, false, 0, false, false, 0, false, false)));
            p := ROW(p.jd - (tx.jd - p.jd), true, 0, 0, 0, false, 0, 0, 0, false, 0, false, 0, true, false, 0, false, false);
        END IF;
        RETURN p;
    ELSIF left(lz, 8) = 'weekday ' THEN
        r := _slonbase.sqlite_number(substr(z, 9));
        IF r IS NULL OR r < 0 OR r >= 7 OR r <> trunc(r) THEN
            RETURN NULL;
        END IF;
        n := r::int;
        p := _slonbase.sqlite_compute_hms(_slonbase.sqlite_compute_ymd(p));
        p.tz := 0;
        p.valid_jd := false;
        p := _slonbase.sqlite_compute_jd(p);
        x := ((p.jd + 129600000) / 86400000 % 7)::int;
        IF x > n THEN
            x := x - 7;
        END IF;
        p.jd := p.jd + (n - x) * 86400000::bigint;
        RETURN _slonbase.sqlite_clear(p);
    ELSIF left(lz, 9) <> 'start of ' AND left(lz, 1) = 's' THEN
        IF lz IN ('subsec', 'subsecond') THEN
            p.subsec := true;
            RETURN p;
        END IF;
        RETURN NULL;
    ELSIF left(lz, 9) = 'start of ' THEN
        IF NOT p.valid_jd AND NOT p.valid_ymd AND NOT p.valid_hms THEN
            RETURN NULL;
        END IF;
        p := _slonbase.sqlite_compute_ymd(p);
        p.valid_hms := true;
        p.h := 0;
        p.mi := 0;
        p.s := 0;
        p.raw_s := false;
        p.tz := 0;
        p.valid_jd := false;
        CASE substr(lz, 10)
            WHEN 'month' THEN p.d := 1;
            WHEN 'year' THEN p.m := 1; p.d := 1;
            WHEN 'day' THEN NULL;
            ELSE RETURN NULL;
        END CASE;
        RETURN p;
    ELSIF left(z, 1) IN ('+', '-', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9') THEN
        -- the number: up to a ':', a space, or the '-' after a (+|-)YYYY or YYYYY
        n := 1;
        WHILE n < length(z) LOOP
            EXIT WHEN substr(z, n + 1, 1) = ':' OR substr(z, n + 1, 1) ~ '[[:space:]]';
            IF substr(z, n + 1, 1) = '-' THEN
                EXIT WHEN n = 5 AND substr(z, 2, 4) ~ '^[0-9]{4}$';
                EXIT WHEN n = 6 AND substr(z, 2, 5) ~ '^[0-9]{5}$';
            END IF;
            n := n + 1;
        END LOOP;
        r := _slonbase.sqlite_number(left(z, n));
        IF r IS NULL THEN
            RETURN NULL;
        END IF;
        z2 := z;
        IF substr(z, n + 1, 1) = '-' THEN
            -- (+|-)YYYY-MM-DD[ HH:MM[:SS[.FFF]]]
            IF left(z, 1) NOT IN ('+', '-') THEN
                RETURN NULL;
            END IF;
            IF n = 5 THEN
                t := regexp_match(substr(z, 2), '^([0-9]{4})-([0-9]{2})-([0-9]{2})');
            ELSE
                t := regexp_match(substr(z, 2), '^([0-9]{5})-([0-9]{2})-([0-9]{2})');
                z2 := substr(z, 2);
            END IF;
            IF t IS NULL OR t[1]::int > 14712 OR t[2]::int >= 12 OR t[3]::int >= 31 THEN
                RETURN NULL;
            END IF;
            p := _slonbase.sqlite_compute_hms(_slonbase.sqlite_compute_ymd(p));
            p.valid_jd := false;
            IF left(z, 1) = '-' THEN
                p.y := p.y - t[1]::int;
                p.m := p.m - t[2]::int;
                x := -t[3]::int;
            ELSE
                p.y := p.y + t[1]::int;
                p.m := p.m + t[2]::int;
                x := t[3]::int;
            END IF;
            n := CASE WHEN p.m > 0 THEN (p.m - 1) / 12 ELSE (p.m - 12) / 12 END;
            p.y := p.y + n;
            p.m := p.m - n * 12;
            p.n_floor := _slonbase.sqlite_floor(p);
            p := _slonbase.sqlite_compute_jd(p);
            p.valid_hms := false;
            p.valid_ymd := false;
            p.jd := p.jd + x * 86400000::bigint;
            IF length(z2) = 11 THEN
                RETURN p;
            END IF;
            IF substr(z2, 12, 1) ~ '[[:space:]]' AND substr(z2, 13) ~ '^[0-9]{2}:[0-9]{2}'
               AND substr(z2, 13, 2)::int <= 24 AND substr(z2, 16, 2)::int <= 59 THEN
                z2 := substr(z2, 13);
                n := 2;
            ELSE
                RETURN NULL;
            END IF;
        END IF;
        IF substr(z2, n + 1, 1) = ':' THEN
            -- (+|-)HH:MM[:SS[.FFF]]
            IF left(z2, 1) !~ '[0-9]' THEN
                z2 := substr(z2, 2);
            END IF;
            tx := _slonbase.sqlite_parse_hms(z2,
                ROW(0, false, 0, 0, 0, false, 0, 0, 0, false, 0, false, 0, false, false, 0, false, false));
            IF tx IS NULL THEN
                RETURN NULL;
            END IF;
            tx := _slonbase.sqlite_compute_jd(tx);
            tx_ms := tx.jd - 43200000;
            tx_ms := tx_ms - (tx_ms / 86400000) * 86400000;
            IF left(z, 1) = '-' THEN
                tx_ms := -tx_ms;
            END IF;
            p := _slonbase.sqlite_clear(_slonbase.sqlite_compute_jd(p));
            p.jd := p.jd + tx_ms;
            RETURN p;
        END IF;
        -- (+|-)NNN days, hours, minutes, seconds, months or years
        unit := lower(ltrim(substr(z, n + 1), E' \t\n\r\f\v'));
        IF length(unit) < 3 OR length(unit) > 10 THEN
            RETURN NULL;
        END IF;
        IF right(unit, 1) = 's' THEN
            unit := left(unit, -1);
        END IF;
        p := _slonbase.sqlite_compute_jd(p);
        rounder := CASE WHEN r < 0 THEN -0.5 ELSE 0.5 END;
        p.n_floor := 0;
        CASE unit
            WHEN 'second' THEN limit_ := 4.6427e+14; xform := 1.0;
            WHEN 'minute' THEN limit_ := 7.7379e+12; xform := 60.0;
            WHEN 'hour' THEN limit_ := 1.2897e+11; xform := 3600.0;
            WHEN 'day' THEN limit_ := 5373485.0; xform := 86400.0;
            WHEN 'month' THEN limit_ := 176546.0; xform := 30.0 * 86400.0;
            WHEN 'year' THEN limit_ := 14713.0; xform := 365.0 * 86400.0;
            ELSE RETURN NULL;
        END CASE;
        IF NOT (r > -limit_ AND r < limit_) THEN
            RETURN NULL;
        END IF;
        IF unit = 'month' THEN
            p := _slonbase.sqlite_compute_hms(_slonbase.sqlite_compute_ymd(p));
            p.m := p.m + trunc(r)::int;
            n := CASE WHEN p.m > 0 THEN (p.m - 1) / 12 ELSE (p.m - 12) / 12 END;
            p.y := p.y + n;
            p.m := p.m - n * 12;
            p.n_floor := _slonbase.sqlite_floor(p);
            p.valid_jd := false;
            r := r - trunc(r);
        ELSIF unit = 'year' THEN
            p := _slonbase.sqlite_compute_hms(_slonbase.sqlite_compute_ymd(p));
            p.y := p.y + trunc(r)::int;
            p.n_floor := _slonbase.sqlite_floor(p);
            p.valid_jd := false;
            r := r - trunc(r);
        END IF;
        p := _slonbase.sqlite_compute_jd(p);
        p.jd := p.jd + trunc(r * 1000.0 * xform + rounder)::bigint;
        RETURN _slonbase.sqlite_clear(p);
    END IF;
    RETURN NULL;
END
$$;

-- C's %0Nd (zero padded, sign first) and %Nd (space padded)
CREATE FUNCTION _slonbase.sqlite_pad(v bigint, width int, fill text) RETURNS text
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN fill = '0' AND v < 0 THEN '-' || lpad((-v)::text, greatest(width - 1, length((-v)::text)), '0')
        ELSE lpad(v::text, greatest(width, length(v::text)), fill)
    END
$$;

-- daysSinceJan1: days since January 1 of the year, at the same time of day
CREATE FUNCTION _slonbase.sqlite_yday(p _slonbase.sqlite_time) RETURNS int
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
    jan1 _slonbase.sqlite_time := p;
BEGIN
    jan1.valid_jd := false;
    jan1.m := 1;
    jan1.d := 1;
    jan1 := _slonbase.sqlite_compute_jd(jan1);
    RETURN ((p.jd - jan1.jd + 43200000) / 86400000)::int;
END
$$;

CREATE FUNCTION _slonbase.strftime(fmt text, val text, mods text[], now_ms bigint) RETURNS text
LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE AS $$
DECLARE
    p _slonbase.sqlite_time;
    w _slonbase.sqlite_time;
    out text := '';
    i int := 1;
    c text;
    h int;
BEGIN
    IF fmt IS NULL OR val IS NULL THEN
        RETURN NULL;
    END IF;
    p := _slonbase.sqlite_parse_time(val, now_ms);
    IF p IS NULL THEN
        RETURN NULL;
    END IF;
    FOR k IN 1 .. coalesce(array_length(mods, 1), 0) LOOP
        IF mods[k] IS NULL THEN
            RETURN NULL;
        END IF;
        p := _slonbase.sqlite_modify(p, mods[k], k);
        IF p IS NULL THEN
            RETURN NULL;
        END IF;
    END LOOP;
    p := _slonbase.sqlite_compute_jd(p);
    IF p.err OR p.jd < 0 OR p.jd > 464269060799999 THEN
        RETURN NULL;
    END IF;
    -- a YYYY-MM-DD without modifiers is normalized (2023-02-31 is 2023-03-03)
    IF coalesce(array_length(mods, 1), 0) = 0 AND p.valid_ymd AND p.d > 28 THEN
        p.valid_ymd := false;
    END IF;
    p := _slonbase.sqlite_compute_hms(_slonbase.sqlite_compute_ymd(p));

    WHILE i <= length(fmt) LOOP
        c := substr(fmt, i, 1);
        IF c <> '%' THEN
            out := out || c;
            i := i + 1;
            CONTINUE;
        END IF;
        c := substr(fmt, i + 1, 1);
        i := i + 2;
        CASE c
            WHEN 'd' THEN out := out || _slonbase.sqlite_pad(p.d, 2, '0');
            WHEN 'e' THEN out := out || _slonbase.sqlite_pad(p.d, 2, ' ');
            WHEN 'f' THEN out := out || lpad(_slonbase.sqlite_printf(least(p.s, 59.999), 3, false), 6, '0');
            WHEN 'F' THEN
                out := out || _slonbase.sqlite_pad(p.y, 4, '0') || '-' || _slonbase.sqlite_pad(p.m, 2, '0')
                    || '-' || _slonbase.sqlite_pad(p.d, 2, '0');
            WHEN 'G', 'g', 'V' THEN
                -- the Thursday of the same week
                w := p;
                w.jd := w.jd + (3 - ((w.jd + 43200000) / 86400000 % 7)) * 86400000;
                w.valid_ymd := false;
                w := _slonbase.sqlite_compute_ymd(w);
                out := out || CASE c
                    WHEN 'G' THEN _slonbase.sqlite_pad(w.y, 4, '0')
                    WHEN 'g' THEN _slonbase.sqlite_pad(w.y % 100, 2, '0')
                    ELSE _slonbase.sqlite_pad(_slonbase.sqlite_yday(w) / 7 + 1, 2, '0')
                END;
            WHEN 'H' THEN out := out || _slonbase.sqlite_pad(p.h, 2, '0');
            WHEN 'k' THEN out := out || _slonbase.sqlite_pad(p.h, 2, ' ');
            WHEN 'I', 'l' THEN
                h := p.h;
                IF h > 12 THEN
                    h := h - 12;
                END IF;
                IF h = 0 THEN
                    h := 12;
                END IF;
                out := out || _slonbase.sqlite_pad(h, 2, CASE c WHEN 'I' THEN '0' ELSE ' ' END);
            WHEN 'j' THEN out := out || _slonbase.sqlite_pad(_slonbase.sqlite_yday(p) + 1, 3, '0');
            WHEN 'W' THEN
                out := out || _slonbase.sqlite_pad(
                    (_slonbase.sqlite_yday(p) + 7 - ((p.jd + 43200000) / 86400000 % 7)::int) / 7, 2, '0');
            WHEN 'U' THEN
                out := out || _slonbase.sqlite_pad(
                    (_slonbase.sqlite_yday(p) + 7 - ((p.jd + 129600000) / 86400000 % 7)::int) / 7, 2, '0');
            WHEN 'J' THEN out := out || _slonbase.sqlite_printf(p.jd / 86400000.0::double precision, 16, true);
            WHEN 'm' THEN out := out || _slonbase.sqlite_pad(p.m, 2, '0');
            WHEN 'M' THEN out := out || _slonbase.sqlite_pad(p.mi, 2, '0');
            WHEN 'p' THEN out := out || CASE WHEN p.h >= 12 THEN 'PM' ELSE 'AM' END;
            WHEN 'P' THEN out := out || CASE WHEN p.h >= 12 THEN 'pm' ELSE 'am' END;
            WHEN 'R' THEN out := out || _slonbase.sqlite_pad(p.h, 2, '0') || ':' || _slonbase.sqlite_pad(p.mi, 2, '0');
            WHEN 's' THEN
                IF p.subsec THEN
                    out := out || _slonbase.sqlite_printf((p.jd - 210866760000000) / 1000.0::double precision, 3, false);
                ELSE
                    out := out || (p.jd / 1000 - 210866760000)::text;
                END IF;
            WHEN 'S' THEN out := out || _slonbase.sqlite_pad(trunc(p.s)::int, 2, '0');
            WHEN 'T' THEN
                out := out || _slonbase.sqlite_pad(p.h, 2, '0') || ':' || _slonbase.sqlite_pad(p.mi, 2, '0')
                    || ':' || _slonbase.sqlite_pad(trunc(p.s)::int, 2, '0');
            WHEN 'u', 'w' THEN
                h := ((p.jd + 129600000) / 86400000 % 7)::int;
                IF h = 0 AND c = 'u' THEN
                    h := 7;
                END IF;
                out := out || h::text;
            WHEN 'Y' THEN
                out := out || _slonbase.sqlite_pad(p.y, 4, '0');
            WHEN '%' THEN out := out || '%';
            ELSE RETURN NULL;
        END CASE;
    END LOOP;
    RETURN out;
END
$$;

-- Apple sends the user's name only with the first redirect (POSTed to /api/oauth2-redirect), not
-- in the id_token: it's kept here for a minute, by authorization code, for the
-- auth-with-oauth2 call that follows, which may reach another instance. Unlogged: a crash
-- loses nothing that matters.
CREATE UNLOGGED TABLE _slonbase.oauth2_names (
    code    TEXT PRIMARY KEY,
    name    TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now()
);

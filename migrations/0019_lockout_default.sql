-- 2.4: the sign-in lockout is on by default (10 failed sign-ins per identity and address in
-- 15 minutes). Installations that have it at 0 are switched on at the next start; the settings
-- may be encrypted, so this only leaves a marker and slonbase does the change
-- (`app::prepare`), then removes the marker.
INSERT INTO _slonbase.params (key, value) VALUES ('upgrade_login_lockout', 'true'::jsonb)
ON CONFLICT (key) DO NOTHING;

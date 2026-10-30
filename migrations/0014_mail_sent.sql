-- When each account was last sent each kind of auth email on request (verification, password
-- reset, email change, OTP), shared by the instances so the one-per-minute throttle holds
-- across them. `sent` is what was sent (an OTP's id). Unlogged: a crash at worst lets one more
-- email through.
CREATE UNLOGGED TABLE _slonbase.mail_sent (
    account TEXT NOT NULL,
    kind    TEXT NOT NULL,
    sent_at TIMESTAMPTZ NOT NULL,
    sent    TEXT NOT NULL,
    PRIMARY KEY (account, kind)
);

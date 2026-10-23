# Security review: authentication

A review of slonbase's authentication code, done in October 2026 before a wider launch. It
found three problems, which are fixed. This page lists them, what was checked and found sound,
and what's left as advice.

To report a problem, see the [security policy](../SECURITY.md).

## Scope

- Tokens: signing, verification, types and expiry, and the request extractor (`src/auth.rs`).
- The auth endpoints: password sign-in, refresh, impersonation, email verification, password
  reset, email change, one-time passwords, MFA and OAuth2 (`src/records/auth_api.rs`,
  `otp.rs`, `oauth2_api.rs`, `src/oauth2.rs`).
- The records API's handling of auth fields (`email`, `password`, `verified`, `tokenKey`).
- The setup wizard's token, rate limits, the superuser IP allowlist and client IP resolution
  (`src/setup.rs`, `src/guard.rs`, `src/ip.rs`).

Each problem was reproduced with a test before it was fixed, and the tests stay in the suite.

## Findings

### 1. The OTP attempt limit could be exceeded with parallel guesses (medium)

An OTP is invalidated after 5 wrong passwords. `auth-with-otp` read the attempt count, checked
the password with bcrypt (tens of milliseconds) and only then counted a wrong one. Guesses sent
at the same time all passed the check before any of them was counted. In the test, 40
simultaneous guesses had 19 checked against one OTP.

Rate limits are off by default, so an attacker who knows a user's email could try far more than
5 codes per OTP, and request up to 9 OTPs at a time.

**Fixed:** each guess takes an attempt in the same statement that checks the limit
(`UPDATE ... WHERE attempts < 5 RETURNING ...`), before the password is checked. Test:
`otp_attempts_hold_under_concurrent_guesses`.

### 2. A client could choose its own IP address behind some proxies (medium)

With a trusted proxy header configured (`trustedProxy.headers`, e.g. `X-Forwarded-For`),
slonbase read only the header's first line. Most proxies append the client's address to the
existing header, but some add a header line of their own instead (HAProxy's `option
forwardfor`, for example). A client that sent its own `X-Forwarded-For` then had its line read,
and could pick any address. That address is what rate limits, the superuser IP allowlist
(`superuserIPs`), logs and auth alerts use.

**Fixed:** every line of the header is read, in order, and the rightmost (or, with
`useLeftmostIP`, leftmost) address across all of them is used. Test: `ip::tests::proxy_headers`.

### 3. Avatar downloads could reach internal addresses (low)

When OAuth2 sign-in creates a record and the collection maps the avatar to a file field,
slonbase downloads the provider's avatar URL. Some identity providers let users set that URL
(e.g. OpenID Connect's `picture` claim). A user could point it at an internal address, such as
a cloud metadata service or an internal HTTP service. The response would be saved as their
avatar, which they can then download.

**Fixed:** avatars are only downloaded from public addresses. Loopback, private, link-local,
shared, reserved and multicast ranges are refused, and so are IPv4-mapped and NAT64 forms of
them. slonbase connects to the address it checked, so a second DNS lookup can't change it.
Redirects are followed by hand (at most 3) and each one is checked the same way. The
provider's own hosts (from its configured URLs) are still allowed, since a superuser chose them
and a self-hosted provider may be on a private network. Tests:
`oauth2::tests::avatars_only_from_public_addresses`, `ip::tests::public_addresses`.

## Checked and sound

- **Tokens** are HS256 JWTs. The key is derived from a 32-byte random secret, plus the
  collection, the token type and the record's `tokenKey`. So a token of one type, collection or
  record can't be used as another, and the `alg` header can't be swapped. Signatures are
  compared in constant time, and expiry is checked. Changing a password or email rotates the
  `tokenKey`, which signs the record out everywhere and makes reset and email change tokens
  single-use.
- **Password sign-in** compares against a real bcrypt hash even when no record matches, so
  response times don't reveal which emails exist. The collection's `authRule` is checked after
  the password. An empty stored hash never matches.
- **Flows started by email** (verification, password reset, OTP) answer the same way whether or
  not the email exists, and send the email in the background. Tokens sent by email are tied to
  the email they were sent to.
- **MFA** sessions are bound to the record and collection, expire, need a different second
  method, and are single-use even when two requests race.
- **OAuth2** only uses emails the provider marks as verified. Microsoft's `mail` isn't used,
  since Microsoft doesn't verify it. An unverified record that matches a provider's verified
  email gets a new password and `tokenKey` before it's marked verified, so whoever created it
  can't keep access. Linking an account that's already linked to another record fails. PKCE is
  on by default.
- **Auth fields in the records API:** only superusers and managers (`manageRule`) can set
  `verified` or change another record's email or password. A record changing its own password
  must send `oldPassword`. The email index is case-insensitive.
- **The setup wizard** needs its token (at least 32 characters, compared in constant time) for
  every step, including the database test, and closes once setup is done.
- **Random values** (secrets, ids, OTPs, token keys) come from a CSPRNG.

## Advice, not changed

- **Turn rate limits on.** They were off by default, as in PocketBase, so nothing limited
  password guessing until a superuser enabled them. Since 0.4.2, new installations start with
  them on (2 sign-in attempts per 3 seconds per client and collection). Older installations keep
  their setting: turn them on in Settings → Rate limits.
- **bcrypt reads only the first 72 bytes of a password.** The default maximum of 71 characters
  stays under that for ASCII, but a password of non-ASCII characters, or a higher maximum, can
  go over it. The bytes past 72 are then ignored.
- **A self-hosted GitLab** with email confirmation turned off reports unconfirmed emails as the
  user's email. Use such an instance as a provider only if it confirms emails.
- **Concurrent password resets with the same token** can both succeed (the last one wins). That
  needs the token, which already allows the reset.

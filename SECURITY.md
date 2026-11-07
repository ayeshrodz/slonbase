# Security policy

## Supported versions

The latest minor release gets security fixes, and the previous minor release gets them for three
months after the next one comes out (see [stability](docs/stability.md#security-fixes)). Upgrade
to the latest before reporting, if you can.

| Version | Supported |
|---|---|
| 2.4.x | ✅ |
| 2.3.x | ✅ until three months after 2.4.0 (February 7, 2027) |
| 2.2.x | ✅ until three months after 2.3.0 (February 7, 2027) |
| 2.1.x | ✅ until three months after 2.2.0 (February 6, 2027) |
| 2.0.x | ✅ until three months after 2.1.0 (February 6, 2027) |
| 1.11.x | ✅ until three months after 2.0.0 (February 6, 2027) |
| 1.10.x | ✅ until three months after 1.11.0 (February 5, 2027) |
| 1.9.x | ✅ until three months after 1.10.0 (February 5, 2027) |
| 1.8.x | ✅ until three months after 1.9.0 (February 4, 2027) |
| 1.7.x | ✅ until three months after 1.8.0 (February 3, 2027) |
| 1.6.x | ✅ until three months after 1.7.0 (February 3, 2027) |
| 1.5.x | ✅ until three months after 1.6.0 (February 2, 2027) |
| 1.4.x | ✅ until three months after 1.5.0 (February 2, 2027) |
| 1.3.x | ✅ until three months after 1.4.0 (February 1, 2027) |
| 1.2.x | ✅ until three months after 1.3.0 (February 1, 2027) |
| 1.1.x | ✅ until three months after 1.2.0 (January 31, 2027) |
| 1.0.x | ✅ until three months after 1.1.0 (January 30, 2027) |
| 0.x | ❌ |

## Reporting a vulnerability

**Please don't open a public issue for a security problem.** Report it privately through GitHub:
**Security → Report a vulnerability** on this repository (see GitHub's guide to
[privately reporting a security vulnerability](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)).

Please include:

- the version, and how slonbase is deployed;
- the steps to reproduce, or a proof of concept;
- the impact as you see it.

You'll get an answer within 7 days. Once the problem is confirmed, a fix is released as soon as
possible, and published together with a GitHub security advisory that credits you (unless you'd
rather not be named). Please give us a reasonable time to fix the problem before you disclose it.

## Published advisories

- [GHSA-8g53-r6rc-fm83](https://github.com/ayeshrodz/slonbase/security/advisories/GHSA-8g53-r6rc-fm83): a restricted dashboard role or API key could escalate to owner or read secrets
  (CVSS 8.1, High). Affects 2.3.1 and earlier; fixed in 2.4.0.

All advisories are listed on the repository's
[Security page](https://github.com/ayeshrodz/slonbase/security/advisories).

## Scope

In scope: the slonbase server, the dashboard, the packages and the Docker image.

Out of scope:

- what a superuser can do by design: the SQL console runs any SQL with the database role's
  privileges, and JavaScript hooks run with the server's;
- problems that need a misconfiguration the documentation warns against, such as trusted proxy
  headers on a server reachable without the proxy;
- denial of service through sheer request volume. Rate limits are available for that.

## Independent review

Reviews of slonbase's security are very welcome, from a quick look at one area to a full
audit. The [threat model](docs/security.md) says what slonbase protects, what it trusts and
where the boundaries are, which is a good place to start. The parts most worth a challenge:

- authorization across list, view, filters, `expand`, files, realtime and batch
  (`src/records/access.rs`, `src/filter/`, checked by `tests/authz_matrix.rs`);
- tokens and their revocation (`src/auth.rs`);
- rate limits and the sign-in lockout with several instances (`src/guard.rs`);
- file serving and uploads (`src/records/files.rs`, `src/storage.rs`).

Findings go through the private report above. Each confirmed one is fixed with a test that keeps
it fixed, listed in the [security reviews](docs/security-review.md), and credited to you.

For hardening advice, see the [security checklist](docs/deployment.md#security-checklist).

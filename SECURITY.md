# Security policy

## Supported versions

slonbase is young, and only the latest release gets security fixes. Upgrade to it before
reporting, if you can.

| Version | Supported |
|---|---|
| 0.2.x | ✅ |
| 0.1.x | ❌ |

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

## Scope

In scope: the slonbase server, the dashboard, the packages and the Docker image.

Out of scope:

- what a superuser can do by design: the SQL console runs any SQL with the database role's
  privileges, and JavaScript hooks run with the server's;
- problems that need a misconfiguration the documentation warns against, such as trusted proxy
  headers on a server reachable without the proxy;
- denial of service through sheer request volume. Rate limits are available for that.

For hardening advice, see the [security checklist](docs/deployment.md#security-checklist).

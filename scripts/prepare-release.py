#!/usr/bin/env python3
"""Prepares a release commit: what a maintainer used to edit by hand before bumping the version.
The Prepare release workflow runs it and opens the result as a pull request; merging that pull
request is the release (see MAINTAINING.md).

    scripts/prepare-release.py <version> [--date YYYY-MM-DD]

It
- sets the version in Cargo.toml (the workflow then refreshes Cargo.lock);
- turns CHANGELOG.md's "Unreleased" section into the version's section (refusing an empty one),
  and rebuilds the compare links at the end of the file;
- for a release on the newest line: points the version pins in the docs and the home page at it;
- for a new minor or major version: adds its line to packaging/supported-lines.txt, gives the
  previous line its end date (three months from the release, the support policy), drops lines
  whose support has ended, and rewrites the table in SECURITY.md to match.
A backport (a patch on an older line) only changes Cargo.toml and CHANGELOG.md.
"""
import argparse
import calendar
import datetime
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parent.parent
REPO = "https://github.com/ayeshrodz/slonbase"
SEMVER = re.compile(r"(\d+)\.(\d+)\.(\d+)")

# Files that pin the current release in examples, and how a pin looks there: the version right
# after one of these characters (v2.5.1, :2.5.1, /v2.5.1/, slonbase-2.5.1-, `2.5.1`, "v2.5.1"),
# so prose such as "releases after 2.5.1" is left alone.
PINNED = [
    "docs/installation.md",
    "docs/extending.md",
    "docs/deployment.md",
    "docs/platforms.md",
    "site/index.html",
]


def parse(v):
    m = SEMVER.fullmatch(v)
    if not m:
        sys.exit(f"{v!r} is not a version like 2.5.2 (pre-releases are tagged by hand)")
    return tuple(int(x) for x in m.groups())


def fmt(v):
    return ".".join(map(str, v))


def add_months(d, months):
    m = d.month - 1 + months
    year, month = d.year + m // 12, m % 12 + 1
    return d.replace(year=year, month=month, day=min(d.day, calendar.monthrange(year, month)[1]))


def long_date(d):
    return f"{d:%B} {d.day}, {d.year}"


def cargo_version():
    m = re.search(r'(?m)^version = "([^"]+)"', (root / "Cargo.toml").read_text())
    return parse(m.group(1))


def set_cargo_version(new):
    path = root / "Cargo.toml"
    text = path.read_text()
    text, n = re.subn(r'(?m)^version = "[^"]+"', f'version = "{fmt(new)}"', text, count=1)
    assert n == 1
    path.write_text(text)


def changelog(new, today):
    path = root / "CHANGELOG.md"
    text = path.read_text()
    if f"## [{fmt(new)}]" in text:
        sys.exit(f"CHANGELOG.md already has a section for {fmt(new)}")
    head = "## [Unreleased]\n"
    start = text.index(head) + len(head)
    nxt = re.search(r"(?m)^## \[", text[start:])
    body = text[start:start + nxt.start()] if nxt else text[start:]
    if not re.search(r"(?m)^- ", body):
        sys.exit("CHANGELOG.md's Unreleased section is empty: there is nothing to release")
    text = text[:start] + f"\n## [{fmt(new)}] - {today.isoformat()}\n" + text[start:]
    # the compare links at the end: one per version, against the next lower version
    links_at = re.search(r"(?m)^\[Unreleased\]: ", text)
    body, _ = (text[:links_at.start()], text[links_at.start():]) if links_at else (text, "")
    versions = sorted({parse(v) for v in re.findall(r"(?m)^## \[(\d+\.\d+\.\d+)\]", body)}, reverse=True)
    links = [f"[Unreleased]: {REPO}/compare/v{fmt(versions[0])}...HEAD"]
    for i, v in enumerate(versions):
        older = versions[i + 1:]
        links.append(f"[{fmt(v)}]: {REPO}/compare/v{fmt(older[0])}...v{fmt(v)}" if older
                     else f"[{fmt(v)}]: {REPO}/releases/tag/v{fmt(v)}")
    path.write_text(body.rstrip("\n") + "\n\n" + "\n".join(links) + "\n")


def pins(old, new):
    pattern = re.compile(r"(?<=[v:/\-`\"])" + re.escape(fmt(old)) + r"(?![\d])")
    changed = []
    for name in PINNED:
        path = root / name
        text = path.read_text()
        out, n = pattern.subn(fmt(new), text)
        if n:
            path.write_text(out)
            changed.append(f"{name} ({n})")
    return changed


def read_lines():
    path = root / "packaging" / "supported-lines.txt"
    header, lines = [], []
    for raw in path.read_text().splitlines():
        text = raw.split("#", 1)[0].strip()
        if not text:
            if not lines:
                header.append(raw)
            continue
        line, until = text.split()
        lines.append((tuple(map(int, line.split("."))),
                      None if until == "-" else datetime.date.fromisoformat(until)))
    return header, lines


def new_line(new, today):
    """A new minor or major: the file and SECURITY.md's table."""
    header, lines = read_lines()
    ends = add_months(today, 3)
    lines = [(line, ends if until is None else until) for line, until in lines]
    lines = [(line, until) for line, until in lines if until is None or until >= today]
    lines.insert(0, ((new[0], new[1]), None))
    width = max(len(f"{a}.{b}") for (a, b), _ in lines)
    body = [f"{a}.{b}".ljust(width + 2) + (until.isoformat() if until else "-")
            for (a, b), until in lines]
    path = root / "packaging" / "supported-lines.txt"
    path.write_text("\n".join(header + body) + "\n")

    rows = []
    for (a, b), until in lines:
        rows.append(f"| {a}.{b}.x | ✅ |" if until is None
                    else f"| {a}.{b}.x | ✅ until three months after the next minor ({long_date(until)}) |")
    oldest = lines[-1][0]
    older = f"{oldest[0]}.{oldest[1] - 1}.x" if oldest[1] > 0 else f"{oldest[0] - 1}.x"
    rows.append(f"| {older} and older | ❌ |")
    table = "| Version | Supported |\n|---|---|\n" + "\n".join(rows) + "\n"
    sec = root / "SECURITY.md"
    text = sec.read_text()
    text, n = re.subn(r"\| Version \| Supported \|\n\|---\|---\|\n(?:\|.*\|\n)+", table, text, count=1)
    if n != 1:
        sys.exit("SECURITY.md: couldn't find the supported versions table")
    sec.write_text(text)
    return lines


def home_page_date(today):
    path = root / "site" / "index.html"
    text = path.read_text()
    text = re.sub(r'(<div class="date" id="rel-date">)[^<]*(</div>)', rf"\g<1>{long_date(today)}\g<2>", text)
    path.write_text(text)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("version")
    ap.add_argument("--date", type=datetime.date.fromisoformat, default=datetime.date.today())
    args = ap.parse_args()
    new, old, today = parse(args.version), cargo_version(), args.date

    _, lines = read_lines()
    newest_line = max(line for line, _ in lines)
    backport = (new[0], new[1]) < newest_line
    if not backport and new <= old:
        sys.exit(f"{fmt(new)} is not newer than the current version {fmt(old)}")
    if backport and (new[0], new[1]) != (old[0], old[1]):
        sys.exit(f"{fmt(new)} is a backport: prepare it on its line's branch (Cargo.toml says {fmt(old)})")

    set_cargo_version(new)
    changelog(new, today)
    summary = [f"Cargo.toml: {fmt(old)} -> {fmt(new)}", f"CHANGELOG.md: section [{fmt(new)}] - {today}"]
    if not backport:
        summary += [f"pins: {', '.join(pins(old, new)) or 'none'}"]
        home_page_date(today)
        if (new[0], new[1]) != (old[0], old[1]):
            kept = new_line(new, today)
            summary.append("supported lines: " + ", ".join(
                f"{a}.{b}" + (f" until {u}" if u else "") for (a, b), u in kept))
    print(("backport " if backport else "") + "release " + fmt(new))
    for line in summary:
        print("  " + line)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Picks the releases whose packages go into the apt and dnf repositories: the newest
`--per-line` versions (default 3) of each release line in packaging/supported-lines.txt whose
support hasn't ended, and always the newest release. Reads the release tags (v2.5.1, ...),
one per line, on stdin; prints the chosen ones, newest first.

    gh release list --json tagName --jq '.[].tagName' | scripts/supported-releases.py
    scripts/supported-releases.py --check     the file is well formed and agrees with SECURITY.md
"""
import argparse
import datetime
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parent.parent
LINES = root / "packaging" / "supported-lines.txt"
SECURITY = root / "SECURITY.md"


def read_lines(path=LINES):
    """{"2.5": None, "2.4": date(2027, 2, 7)}: each line and its last day (None: open)."""
    lines = {}
    for n, raw in enumerate(path.read_text().splitlines(), 1):
        text = raw.split("#", 1)[0].strip()
        if not text:
            continue
        parts = text.split()
        if len(parts) != 2 or not re.fullmatch(r"\d+\.\d+", parts[0]):
            sys.exit(f"{path}:{n}: expected '<major.minor> <YYYY-MM-DD or ->', got {raw!r}")
        line, until = parts
        if line in lines:
            sys.exit(f"{path}:{n}: {line} is listed twice")
        try:
            lines[line] = None if until == "-" else datetime.date.fromisoformat(until)
        except ValueError:
            sys.exit(f"{path}:{n}: {until!r} is not a date (YYYY-MM-DD) or -")
    if not lines:
        sys.exit(f"{path}: no release lines")
    return lines


def version(tag):
    m = re.fullmatch(r"v?(\d+)\.(\d+)\.(\d+)", tag.strip())
    return tuple(int(x) for x in m.groups()) if m else None  # pre-releases and others: skipped


def choose(tags, lines, today, per_line=3):
    versions = sorted({v for v in map(version, tags) if v}, reverse=True)
    if not versions:
        return []
    live = {line for line, until in lines.items() if until is None or until >= today}
    chosen, count = [], {}
    for v in versions:
        line = f"{v[0]}.{v[1]}"
        if line in live and count.get(line, 0) < per_line:
            count[line] = count.get(line, 0) + 1
            chosen.append(v)
    if versions[0] not in chosen:
        print(f"warning: the newest release {'.'.join(map(str, versions[0]))} is on a line not "
              f"in {LINES.name}; keeping it anyway", file=sys.stderr)
        chosen.insert(0, versions[0])
    return ["v" + ".".join(map(str, v)) for v in sorted(chosen, reverse=True)]


def check():
    lines = read_lines()
    table = SECURITY.read_text()
    problems = []
    for line in lines:
        row = re.search(rf"^\|\s*{re.escape(line)}\.x\s*\|(.*)\|\s*$", table, re.M)
        if not row or "✅" not in row.group(1):
            problems.append(f"{line} is in {LINES.name} but not marked supported (✅) in SECURITY.md")
    open_lines = [line for line, until in lines.items() if until is None]
    if len(open_lines) != 1:
        problems.append(f"exactly one line should have no end date ('-'), found {open_lines}")
    for p in problems:
        print(f"error: {p}", file=sys.stderr)
    if problems:
        sys.exit(1)
    print(f"{LINES.name}: {', '.join(lines)} agree with SECURITY.md")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--per-line", type=int, default=3)
    ap.add_argument("--today", type=datetime.date.fromisoformat, default=datetime.date.today())
    args = ap.parse_args()
    if args.check:
        return check()
    for tag in choose(sys.stdin.read().split(), read_lines(), args.today, args.per_line):
        print(tag)


if __name__ == "__main__":
    main()

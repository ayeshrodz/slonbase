#!/usr/bin/env python3
"""Runs scripts/prepare-release.py on a copy of the files it changes, for a release, release
candidates and the release after them, and checks what it wrote. CI runs it.

    python3 scripts/test-prepare-release.py
"""
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

root = pathlib.Path(__file__).resolve().parent.parent
FILES = [
    "Cargo.toml", "CHANGELOG.md", "SECURITY.md", "packaging/supported-lines.txt",
    "docs/installation.md", "docs/extending.md", "docs/deployment.md", "docs/platforms.md",
    "site/index.html", "scripts/prepare-release.py",
]
failures = []


def check(ok, what):
    if not ok:
        failures.append(what)
        print(f"FAIL: {what}", file=sys.stderr)


def copy():
    tmp = pathlib.Path(tempfile.mkdtemp())
    for name in FILES:
        (tmp / name).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(root / name, tmp / name)
    return tmp


def run(tmp, version, date="2027-01-15", ok=True):
    done = subprocess.run([sys.executable, str(tmp / "scripts/prepare-release.py"), version, "--date", date],
                          capture_output=True, text=True)
    check((done.returncode == 0) == ok, f"{version}: exit {done.returncode}: {done.stdout}{done.stderr}")
    return done.stdout + done.stderr


def read(tmp, name):
    return (tmp / name).read_text()


def cargo(tmp):
    return re.search(r'(?m)^version = "([^"]+)"', read(tmp, "Cargo.toml")).group(1)


def section(text, version):
    """The body of a changelog section, or None."""
    m = re.search(rf"(?ms)^## \[{re.escape(version)}\][^\n]*\n(.*?)(?=^## \[|^\[Unreleased\]: )", text)
    return m.group(1).strip() if m else None


def main():
    base = cargo(copy())
    major, minor, _ = map(int, base.split("-")[0].split("."))
    if "-" in base:
        print(f"Cargo.toml is at a release candidate ({base}): nothing to check from here")
        return
    nxt = f"{major}.{minor + 1}.0"
    lines_before = read(copy(), "packaging/supported-lines.txt")

    # a release candidate, a second one, then the release
    tmp = copy()
    unreleased = section(read(tmp, "CHANGELOG.md"), "Unreleased")
    check(unreleased, "the copied changelog has Unreleased notes to release")
    out = run(tmp, f"{nxt}-rc.1")
    log = read(tmp, "CHANGELOG.md")
    check(cargo(tmp) == f"{nxt}-rc.1", f"Cargo.toml says {nxt}-rc.1: {cargo(tmp)}")
    check(section(log, f"{nxt}-rc.1") == unreleased, "rc.1's section has the Unreleased notes")
    check(section(log, "Unreleased") == unreleased, "the notes stay under Unreleased")
    check(f"[{nxt}-rc.1]: " in log and f"v{base}...v{nxt}-rc.1" in log, f"rc.1's compare link: {out}")
    check(read(tmp, "packaging/supported-lines.txt") == lines_before, "a candidate adds no supported line")
    check(base in read(tmp, "docs/installation.md") and f"{nxt}-rc" not in read(tmp, "docs/installation.md"),
          "a candidate leaves the pins alone")
    run(tmp, f"{nxt}-rc.1", ok=False)  # the same one twice
    run(tmp, f"{nxt}-rc.2", date="2027-01-20")
    log = read(tmp, "CHANGELOG.md")
    check(log.index(f"## [{nxt}-rc.2]") < log.index(f"## [{nxt}-rc.1]"), "rc.2 comes above rc.1")
    check(section(log, "Unreleased") == unreleased, "the notes still stay under Unreleased")

    out = run(tmp, nxt, date="2027-01-25")
    log = read(tmp, "CHANGELOG.md")
    check(cargo(tmp) == nxt, f"Cargo.toml says {nxt}")
    check(f"-rc." not in log.split("[Unreleased]: ")[0], "the release drops its candidates' sections")
    check(section(log, nxt) == unreleased, "the release's section has the notes")
    check(section(log, "Unreleased") == "", "Unreleased is empty after the release")
    check(f"[{nxt}]: " in log and f"v{base}...v{nxt}" in log, "the release compares with the one before")
    check(f"{major}.{minor + 1}  " in read(tmp, "packaging/supported-lines.txt"),
          f"the new line is supported: {read(tmp, 'packaging/supported-lines.txt')}")
    check(nxt in read(tmp, "docs/installation.md"), f"the pins move to {nxt}: {out}")

    # a release without candidates, as before
    tmp = copy()
    run(tmp, nxt)
    log = read(tmp, "CHANGELOG.md")
    check(section(log, nxt) == unreleased and section(log, "Unreleased") == "", "a plain release")
    check(nxt in read(tmp, "docs/installation.md"), "a plain release moves the pins")

    # what is refused
    tmp = copy()
    run(tmp, f"{base}-rc.1", ok=False)       # older than what Cargo.toml says
    run(tmp, f"{nxt}-beta.1", ok=False)      # only -rc.N
    run(tmp, f"{nxt}-rc.0", ok=False)
    run(tmp, f"{nxt}-rc.1")
    run(tmp, f"{nxt}-rc.1", ok=False)        # no older than the candidate it is at

    if failures:
        sys.exit(f"{len(failures)} check(s) failed")
    print("prepare-release: candidates and releases as expected")


if __name__ == "__main__":
    main()

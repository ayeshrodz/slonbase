#!/usr/bin/env python3
"""Writes a software bill of materials (SPDX 2.3, JSON) for a slonbase release: what the
binary is built from, so that a user or a scanner can check it against vulnerability databases.

It lists
- the Rust crates compiled into the binary on Linux (x86_64 and aarch64, musl): the normal
  dependencies reachable from slonbase in Cargo.lock, not build scripts' or tests' crates,
  each with its version, license, crates.io address and the checksum Cargo.lock records;
- what the dashboard bundles: Preact, htm and the two fonts.

The release workflow publishes it next to the packages and attests it (see MAINTAINING.md).

    scripts/sbom.py <version> [<output>]     (default output: stdout)
"""
import datetime
import hashlib
import json
import pathlib
import subprocess
import sys
import tomllib
import uuid

root = pathlib.Path(__file__).resolve().parent.parent
TARGETS = ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"]
REPO = "https://github.com/ayeshrodz/slonbase"

# What the dashboard ships from outside, with the file it is in. A vendored file's version is
# found by comparing it with the published npm packages: preact.js is preact's
# dist/preact.module.js, hooks.js its hooks/dist/hooks.module.js with the import pointed at
# ./preact.js, htm.js htm's dist/htm.module.js (the same file from 3.0.1 to 3.1.1). Update this
# list with the files; CI checks that each file is still the one listed.
BUNDLED = [
    {"name": "preact", "version": "10.29.8", "license": "MIT", "file": "ui/app/vendor/preact.js",
     "sha256": "c30e721ebfdc6e2ad4c18c14d2dfb82667829c8aec27de1207774e3fc16858a8",
     "purl": "pkg:npm/preact@10.29.8", "home": "https://preactjs.com"},
    {"name": "preact/hooks", "version": "10.29.8", "license": "MIT", "file": "ui/app/vendor/hooks.js",
     "sha256": "960d4e1123fae82515f94d23ec1b6b9adff539db9a83418e52264593d82cdcdf",
     "purl": "pkg:npm/preact@10.29.8#hooks", "home": "https://preactjs.com"},
    {"name": "htm", "version": "3.1.1", "license": "Apache-2.0", "file": "ui/app/vendor/htm.js",
     "sha256": "ab33dd3f38059b9be4d5f5350128eefb2356639c4e0bbe9d9e8b3ba75847e9e4",
     "purl": "pkg:npm/htm@3.1.1", "home": "https://github.com/developit/htm"},
    {"name": "IBM Plex Sans", "version": None, "license": "OFL-1.1",
     "file": "ui/app/fonts/ibm-plex-sans-latin.woff2",
     "sha256": "1e875856600a34cb44a406a7deef0e3f9ae70e6f636e0f37fbca2248dec2dca4", "purl": None,
     "home": "https://github.com/IBM/plex"},
    {"name": "JetBrains Mono", "version": None, "license": "OFL-1.1",
     "file": "ui/app/fonts/jetbrains-mono-latin.woff2",
     "sha256": "18be452724bfdc236c074ca94a249a7f41a86752c7d04ab258ce9ed5651f6a7e", "purl": None,
     "home": "https://github.com/JetBrains/JetBrainsMono"},
]


def spdx_id(text: str) -> str:
    return "SPDXRef-" + "".join(c if c.isalnum() or c in ".-" else "-" for c in text)


def license_expr(raw):
    # Cargo still accepts the old "MIT/Apache-2.0" form; SPDX wants an expression
    if not raw:
        return "NOASSERTION"
    return " OR ".join(part.strip() for part in raw.split("/")) if "/" in raw else raw


def shipped_crates():
    """Packages reachable from the root through normal dependencies, on the release targets."""
    seen = {}
    for target in TARGETS:
        meta = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target],
            cwd=root, text=True))
        packages = {p["id"]: p for p in meta["packages"]}
        nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
        stack = [meta["resolve"]["root"]]
        reached = set()
        while stack:
            node = nodes[stack.pop()]
            if node["id"] in reached:
                continue
            reached.add(node["id"])
            for dep in node["deps"]:
                if any(k["kind"] is None for k in dep["dep_kinds"]):
                    stack.append(dep["pkg"])
        for pid in reached:
            seen[pid] = packages[pid]
    return seen


def build(version: str) -> dict:
    lock = tomllib.loads((root / "Cargo.lock").read_text())
    checksums = {(p["name"], p["version"]): p.get("checksum") for p in lock["package"]}
    crates = shipped_crates()
    root_pkg = next(p for p in crates.values() if p["source"] is None and p["name"] == "slonbase")

    main_id = "SPDXRef-Package-slonbase"
    packages = [{
        "SPDXID": main_id,
        "name": "slonbase",
        "versionInfo": version,
        "supplier": "Person: Ayesh Rodrigo",
        "downloadLocation": f"git+{REPO}@v{version}",
        "homepage": "https://slonbase.com",
        "licenseConcluded": "Apache-2.0",
        "licenseDeclared": license_expr(root_pkg.get("license")),
        "copyrightText": "Copyright 2026 Ayesh Rodrigo",
        "primaryPackagePurpose": "APPLICATION",
        "externalRefs": [{"referenceCategory": "PACKAGE-MANAGER", "referenceType": "purl",
                          "referenceLocator": f"pkg:github/ayeshrodz/slonbase@v{version}"}],
    }]
    relationships = [{"spdxElementId": "SPDXRef-DOCUMENT", "relationshipType": "DESCRIBES",
                      "relatedSpdxElement": main_id}]

    for pkg in sorted(crates.values(), key=lambda p: (p["name"], p["version"])):
        if pkg["source"] is None:
            continue  # slonbase itself
        name, ver = pkg["name"], pkg["version"]
        entry = {
            "SPDXID": spdx_id(f"Package-crate-{name}-{ver}"),
            "name": name,
            "versionInfo": ver,
            "downloadLocation": f"https://crates.io/api/v1/crates/{name}/{ver}/download",
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": license_expr(pkg.get("license")),
            "copyrightText": "NOASSERTION",
            "primaryPackagePurpose": "LIBRARY",
            "externalRefs": [{"referenceCategory": "PACKAGE-MANAGER", "referenceType": "purl",
                              "referenceLocator": f"pkg:cargo/{name}@{ver}"}],
        }
        if pkg.get("repository"):
            entry["homepage"] = pkg["repository"]
        if checksums.get((name, ver)):
            entry["checksums"] = [{"algorithm": "SHA256", "checksumValue": checksums[(name, ver)]}]
        packages.append(entry)
        relationships.append({"spdxElementId": main_id, "relationshipType": "CONTAINS",
                              "relatedSpdxElement": entry["SPDXID"]})

    for item in BUNDLED:
        digest = hashlib.sha256((root / item["file"]).read_bytes()).hexdigest()
        if digest != item["sha256"]:
            sys.exit(f"{item['file']} changed: update its entry in BUNDLED (version, sha256)")
        entry = {
            "SPDXID": spdx_id(f"Package-bundled-{item['name']}"),
            "name": item["name"],
            "downloadLocation": "NOASSERTION",
            "homepage": item["home"],
            "licenseConcluded": item["license"],
            "licenseDeclared": item["license"],
            "copyrightText": "NOASSERTION",
            "primaryPackagePurpose": "LIBRARY" if item["purl"] else "OTHER",
            "checksums": [{"algorithm": "SHA256", "checksumValue": digest}],
            "comment": f"bundled in the dashboard as {item['file']}",
        }
        if item["version"]:
            entry["versionInfo"] = item["version"]
        if item["purl"]:
            entry["externalRefs"] = [{"referenceCategory": "PACKAGE-MANAGER",
                                      "referenceType": "purl", "referenceLocator": item["purl"]}]
        packages.append(entry)
        relationships.append({"spdxElementId": main_id, "relationshipType": "CONTAINS",
                              "relatedSpdxElement": entry["SPDXID"]})

    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    return {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"slonbase-{version}",
        "documentNamespace": f"{REPO}/releases/v{version}/sbom-{uuid.uuid4()}",
        "creationInfo": {"created": now, "creators": ["Tool: slonbase scripts/sbom.py"]},
        "packages": packages,
        "relationships": relationships,
    }


def main():
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    text = json.dumps(build(sys.argv[1]), indent=1) + "\n"
    if len(sys.argv) == 3:
        pathlib.Path(sys.argv[2]).write_text(text)
    else:
        sys.stdout.write(text)


if __name__ == "__main__":
    main()

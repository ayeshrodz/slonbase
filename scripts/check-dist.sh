#!/bin/sh
# Checks what a release ships, before it is published: every archive and package carries the
# license, the notice and the third-party licenses, and the binary is executable and reports the
# version being released.
#
#   scripts/check-dist.sh dist 2.5.1
set -eu

dist=${1:?usage: check-dist.sh <dist dir> <version>}
version=${2:?usage: check-dist.sh <dist dir> <version>}
fail=0
bad() { echo "check-dist: $*" >&2; fail=1; }

archives=0
for tarball in "$dist"/slonbase-"$version"-linux-*.tar.gz; do
    [ -e "$tarball" ] || continue
    archives=$((archives + 1))
    list=$(tar tzf "$tarball")
    for f in slonbase LICENSE NOTICE THIRD-PARTY-LICENSES.md README.md; do
        echo "$list" | grep -qx "$f" || bad "$tarball lacks $f"
    done
done
[ "$archives" -gt 0 ] || bad "no tarball for $version in $dist"

packages=0
for deb in "$dist"/slonbase_"${version}"_*.deb; do
    [ -e "$deb" ] || continue
    packages=$((packages + 1))
    list=$(dpkg-deb -c "$deb")
    for f in usr/bin/slonbase usr/share/doc/slonbase/copyright usr/share/doc/slonbase/LICENSE \
        usr/share/doc/slonbase/NOTICE usr/share/doc/slonbase/THIRD-PARTY-LICENSES.md \
        usr/lib/systemd/system/slonbase.service etc/default/slonbase; do
        echo "$list" | grep -q " \./$f\$" || bad "$deb lacks /$f"
    done
    [ "$(dpkg-deb -f "$deb" Version)" = "$version" ] || bad "$deb has the wrong version"
done
for rpm in "$dist"/slonbase-"${version}"-1.*.rpm; do
    [ -e "$rpm" ] || continue
    packages=$((packages + 1))
    list=$(rpm -qpl "$rpm")
    for f in usr/bin/slonbase usr/share/doc/slonbase/copyright usr/share/doc/slonbase/LICENSE \
        usr/share/doc/slonbase/NOTICE usr/share/doc/slonbase/THIRD-PARTY-LICENSES.md \
        usr/lib/systemd/system/slonbase.service etc/default/slonbase; do
        echo "$list" | grep -qx "/$f" || bad "$rpm lacks /$f"
    done
    [ "$(rpm -qp --qf '%{VERSION}' "$rpm")" = "$version" ] || bad "$rpm has the wrong version"
    [ "$(rpm -qp --qf '%{LICENSE}' "$rpm")" = "Apache-2.0" ] || bad "$rpm isn't licensed Apache-2.0"
done
[ "$packages" -gt 0 ] || bad "no .deb or .rpm for $version in $dist"

# the binary of this machine's architecture runs and reports the version
for tarball in "$dist"/slonbase-"$version"-linux-*.tar.gz; do
    [ -e "$tarball" ] || continue
    case "$tarball" in
        *-amd64.tar.gz) want=x86_64 ;;
        *-arm64.tar.gz) want=aarch64 ;;
        *) continue ;;
    esac
    [ "$(uname -m)" = "$want" ] || continue
    tmp=$(mktemp -d)
    tar xzf "$tarball" -C "$tmp" slonbase
    out=$("$tmp/slonbase" --version 2>&1) || bad "$tarball: the binary doesn't run"
    echo "$out" | grep -q "$version" || bad "$tarball: the binary says '$out', not $version"
    rm -rf "$tmp"
done

if [ "$fail" -ne 0 ]; then
    exit 1
fi
echo "check-dist: $archives archives and $packages packages for $version look right"

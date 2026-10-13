#!/bin/sh
# Packages an already built release binary into .deb, .rpm and .tar.gz files in dist/.
#
#   cargo build --release --locked --target x86_64-unknown-linux-musl
#   scripts/package.sh x86_64-unknown-linux-musl amd64 0.1.0
#
# Requires nfpm (https://nfpm.goreleaser.com).
set -eu

if [ $# -ne 3 ]; then
    echo "usage: $0 <rust-target> <arch: amd64|arm64> <version>" >&2
    exit 2
fi
target=$1
arch=$2
version=$3
root=$(pwd)

mkdir -p dist "dist/linux-$arch"
cp "target/$target/release/slonbase" dist/slonbase

for format in deb rpm; do
    VERSION=$version ARCH=$arch nfpm package -f packaging/nfpm.yaml -p "$format" -t dist/
done

mv dist/slonbase "dist/linux-$arch/slonbase"
tar -czf "dist/slonbase-$version-linux-$arch.tar.gz" \
    -C "$root/dist/linux-$arch" slonbase \
    -C "$root" LICENSE README.md

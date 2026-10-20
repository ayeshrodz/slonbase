#!/bin/sh
# Builds signed apt and dnf repositories from .deb and .rpm files, ready to serve as static
# files (the Package repositories workflow publishes them on GitHub Pages).
#
#   scripts/build-repo.sh <packages-dir> <out-dir> <gpg-key> <base-url>
#
# <gpg-key> is a secret key in the gpg keyring without a passphrase. Needs apt-ftparchive
# (apt-utils), createrepo_c and rpmsign (rpm).
set -eu

if [ $# -ne 4 ]; then
    echo "usage: $0 <packages-dir> <out-dir> <gpg-key> <base-url>" >&2
    exit 2
fi
packages=$(cd "$1" && pwd)
out=$2
key=$3
url=${4%/}

rm -rf "$out"
mkdir -p "$out"
out=$(cd "$out" && pwd)

# ---------------------------------------------------------------- apt
apt="$out/apt"
mkdir -p "$apt/pool/main"
cp "$packages"/*.deb "$apt/pool/main/"
for arch in amd64 arm64; do
    dir="$apt/dists/stable/main/binary-$arch"
    mkdir -p "$dir"
    (cd "$apt" && apt-ftparchive --arch "$arch" packages pool) > "$dir/Packages"
    gzip -9kn "$dir/Packages"
done
release=$(mktemp)
apt-ftparchive \
    -o APT::FTPArchive::Release::Origin=slonbase \
    -o APT::FTPArchive::Release::Label=slonbase \
    -o APT::FTPArchive::Release::Suite=stable \
    -o APT::FTPArchive::Release::Codename=stable \
    -o APT::FTPArchive::Release::Architectures="amd64 arm64" \
    -o APT::FTPArchive::Release::Components=main \
    -o APT::FTPArchive::Release::Description="slonbase packages" \
    release "$apt/dists/stable" > "$release"
mv "$release" "$apt/dists/stable/Release"
gpg --batch --yes --local-user "$key" --clearsign \
    -o "$apt/dists/stable/InRelease" "$apt/dists/stable/Release"
gpg --batch --yes --local-user "$key" --armor --detach-sign \
    -o "$apt/dists/stable/Release.gpg" "$apt/dists/stable/Release"

# ---------------------------------------------------------------- dnf
rpm="$out/rpm"
mkdir -p "$rpm"
cp "$packages"/*.rpm "$rpm/"
# rpmsign warns that it can't set GPG_TTY when there's no terminal; only show its output on failure
if ! log=$(rpmsign --define "_gpg_name $key" --addsign "$rpm"/*.rpm 2>&1); then
    echo "$log" >&2
    exit 1
fi
createrepo_c --quiet "$rpm"
gpg --batch --yes --local-user "$key" --armor --detach-sign "$rpm/repodata/repomd.xml"

# ---------------------------------------------------------------- key and client files
gpg --armor --export "$key" > "$out/slonbase.asc"
gpg --export "$key" > "$out/slonbase.gpg"
cat > "$out/slonbase.list" <<EOF
deb [arch=amd64,arm64 signed-by=/usr/share/keyrings/slonbase.gpg] $url/apt stable main
EOF
cat > "$out/slonbase.repo" <<EOF
[slonbase]
name=slonbase
baseurl=$url/rpm
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=$url/slonbase.asc
EOF

versions=$(ls "$apt/pool/main" | sed -n 's/^slonbase_\([^_]*\)_.*/\1/p' | sort -uV | tr '\n' ' ')
fingerprint=$(gpg --with-colons --fingerprint "$key" | awk -F: '/^fpr/ {print $10; exit}')
cat > "$out/index.html" <<EOF
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>slonbase packages</title>
<style>
  :root { color-scheme: light dark; }
  body { font: 15px/1.6 system-ui, sans-serif; max-width: 760px; margin: 48px auto; padding: 0 16px; }
  pre { background: color-mix(in srgb, currentColor 7%, transparent); padding: 12px 14px; border-radius: 8px; overflow-x: auto; }
  code { font-size: 13px; }
  .muted { opacity: 0.7; }
</style>
</head>
<body>
<h1>slonbase packages</h1>
<p>Signed apt and dnf repositories for <a href="https://github.com/ayeshrodz/slonbase">slonbase</a>.
Versions: $versions</p>

<h2>Debian, Ubuntu</h2>
<pre><code>curl -fsSL $url/slonbase.gpg | sudo tee /usr/share/keyrings/slonbase.gpg &gt; /dev/null
curl -fsSL $url/slonbase.list | sudo tee /etc/apt/sources.list.d/slonbase.list
sudo apt update &amp;&amp; sudo apt install slonbase</code></pre>

<h2>Fedora, RHEL, Rocky, Alma</h2>
<pre><code>sudo curl -fsSL -o /etc/yum.repos.d/slonbase.repo $url/slonbase.repo
sudo dnf install slonbase</code></pre>

<p>Updates then arrive with <code>apt upgrade</code> or <code>dnf upgrade</code>.</p>
<p class="muted">Signing key fingerprint: <code>$fingerprint</code></p>
</body>
</html>
EOF

echo "built $out: $(ls "$apt/pool/main" | wc -l) .deb, $(ls "$rpm"/*.rpm | wc -l) .rpm (versions: $versions)"

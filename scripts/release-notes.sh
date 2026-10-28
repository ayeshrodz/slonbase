#!/bin/sh
# Prints the CHANGELOG.md section of a version (without its heading), for the release notes.
# Links to files in the repository become links to them at the release's tag, since relative
# links don't work on a release page.
#   scripts/release-notes.sh 0.1.0
set -eu
version="$1"
notes=$(awk -v v="$version" '
    index($0, "## [" v "]") == 1 { found = 1; next }
    found && /^## \[/ { exit }
    found && /^\[[^]]+\]: / { exit }
    found { print }
' CHANGELOG.md | sed -e '/./,$!d' \
    -e "s#](\([^)#:/][^):]*\))#](https://github.com/ayeshrodz/slonbase/blob/v$version/\1)#g")
if [ -z "$notes" ]; then
    echo "CHANGELOG.md has no section for $version" >&2
    exit 1
fi
printf '%s\n' "$notes"

#!/bin/sh
# Runs before removal (deb: "remove"/"upgrade", rpm: $1 = 0 remove / 1 upgrade).
set -e

case "$1" in
    remove|0)
        if [ -d /run/systemd/system ]; then
            systemctl stop slonbase.service >/dev/null 2>&1 || true
            systemctl disable slonbase.service >/dev/null 2>&1 || true
        fi
        ;;
esac

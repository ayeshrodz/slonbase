#!/bin/sh
# Runs after removal. On "purge" (deb only) the config and local data are deleted.
# The PostgreSQL database is never touched.
set -e

if [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || true
fi

if [ "$1" = "purge" ]; then
    rm -rf /etc/slonbase /var/lib/slonbase
fi

#!/bin/sh
# Runs after install and upgrade (deb: "configure", rpm: $1 = 1 install / 2 upgrade).
set -e

if ! getent group slonbase >/dev/null 2>&1; then
    groupadd --system slonbase
fi
if ! getent passwd slonbase >/dev/null 2>&1; then
    useradd --system --gid slonbase --home-dir /var/lib/slonbase --no-create-home \
        --shell "$(command -v nologin || echo /bin/false)" \
        --comment "slonbase service" slonbase
fi

chown slonbase:slonbase /etc/slonbase /var/lib/slonbase
chmod 0750 /etc/slonbase /var/lib/slonbase
if [ -f /etc/slonbase/config.toml ]; then
    chown slonbase:slonbase /etc/slonbase/config.toml
    chmod 0600 /etc/slonbase/config.toml
fi

if [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null 2>&1 || true
    systemctl enable slonbase.service >/dev/null 2>&1 || true
    # starts on a fresh install, picks up the new binary on upgrade
    systemctl restart slonbase.service >/dev/null 2>&1 || true
fi

if [ ! -f /etc/slonbase/config.toml ]; then
    cat <<'MSG'

slonbase is installed. To finish setting it up, get the one-time setup link:

    sudo journalctl -u slonbase | grep "setup?token"

The server listens on 127.0.0.1:8090. From another machine, use an SSH tunnel
(ssh -L 8090:127.0.0.1:8090 <server>) or set SLONBASE_BIND in /etc/default/slonbase.

Or set it up from the command line:

    sudo -u slonbase env SLONBASE_SUPERUSER_PASSWORD=... slonbase setup \
        --config /etc/slonbase/config.toml \
        --database-url 'postgres://user:pass@host:5432/db' --email you@example.com

MSG
fi

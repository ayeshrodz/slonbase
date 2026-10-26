# The slonbase image, run as root: Render mounts its disk at /data owned by root, which the
# image's unprivileged user can't write to. Pin a release (e.g. :0.6.0) to control upgrades.
FROM ghcr.io/ayeshrodz/slonbase:latest
USER 0:0

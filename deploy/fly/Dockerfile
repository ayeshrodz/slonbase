# The slonbase image, run as root: Fly mounts its volume at /data owned by root, which the
# image's unprivileged user can't write to. Pin a release (e.g. :0.4.1) to control upgrades.
FROM ghcr.io/ayeshrodz/slonbase:latest
USER 0:0

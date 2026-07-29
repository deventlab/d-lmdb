#!/bin/sh
# Runs as root on container start so it can fix ownership of a bind-mounted
# /data whose host-side UID/GID may not match appuser's fixed 1000:1000 —
# then drops privileges and re-execs itself, so the real server process never
# runs as root.
set -e

if [ "$(id -u)" = "0" ]; then
    chown -R appuser:appuser /data
    exec gosu appuser "$0" "$@"
fi

exec "$@"

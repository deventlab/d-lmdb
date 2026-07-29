#!/bin/sh
# Fix ownership of bind-mounted /data, then drop to appuser via setpriv.
set -e

if [ "$(id -u)" = "0" ]; then
    # Only chown if the mount point isn't already owned by appuser.
    # Unconditional chown -R gets expensive as LMDB data grows, eating into
    # the HEALTHCHECK start-period budget (measured at ~863ms on cold start).
    if [ "$(stat -c '%u:%g' /data 2>/dev/null)" != "1000:1000" ]; then
        chown -R appuser:appuser /data
    fi
    exec setpriv --reuid=appuser --regid=appuser --init-groups "$0" "$@"
fi

exec "$@"

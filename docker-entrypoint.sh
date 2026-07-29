#!/bin/sh
set -e

# Generate a single-node config if none is mounted.
CONFIG_FILE="${CONFIG:-/etc/dlmdb/config.toml}"
if [ ! -f "$CONFIG_FILE" ]; then
    mkdir -p "$(dirname "$CONFIG_FILE")"
    cat > "$CONFIG_FILE" << CFGEOF
[cluster]
node_id = 1
listen_address = "0.0.0.0:9081"
db_root_dir = "/data/raft"
log_dir = "/data/logs"

[lmdb]
data_dir = "/data/lmdb"
map_size_gb = 1

[http]
listen_address = "0.0.0.0:8080"
CFGEOF
fi

# Inject --config unless already provided via docker run args.
case " $* " in
    *" --config "*) ;;
    *) set -- "$@" --config "$CONFIG_FILE" ;;
esac

if [ "$(id -u)" = "0" ]; then
    if [ "$(stat -c %u:%g /data 2>/dev/null)" != "1000:1000" ]; then
        chown -R appuser:appuser /data
    fi
    exec setpriv --reuid=appuser --regid=appuser --init-groups "$0" "$@"
fi

exec "$@"

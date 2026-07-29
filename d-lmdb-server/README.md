# d-lmdb-server

---

d-lmdb-server is for developers who want distributed, LMDB-backed key-value
storage as a standalone service — no Rust, no embedding, no client library.

It wraps [d-lmdb](../d-lmdb) behind HTTP+JSON: writes go through Raft
consensus and are replicated across nodes, reads stay local by default.

Run it as a standalone service and talk to it over plain HTTP from any language.

> **Docker image**: `deventlab/d-lmdb` | **Service name**: `d-lmdb-server`
> The image name is kept short; the service name matches the server binary.

## Quick Start

### Single node

```bash
docker rm -f dlmdb 2>/dev/null   # clean up from a previous run
docker run -d --name dlmdb -p 8080:8080 -v dlmdb-data:/data deventlab/d-lmdb
sleep 3                            # wait for bootstrap
curl -s -o /dev/null -w 'PUT: %{http_code}\n' -X PUT localhost:8080/kv/hello -d world
# PUT: 204
curl localhost:8080/kv/hello
# world
```

No config needed for the first run. A single-node TOML is auto-generated
on first start. Mount a custom config at `/etc/dlmdb/config.toml` or set
`-e CONFIG=/path` to override.

### 3-node HA cluster

```bash
git clone https://github.com/deventlab/d-lmdb.git && cd d-lmdb
docker pull deventlab/d-lmdb:latest
docker tag deventlab/d-lmdb:latest d-lmdb-server:compose   # compose uses this local tag
docker compose down -v 2>/dev/null
docker compose up -d
sleep 10   # wait for leader election + HAProxy health checks
```

**Verify the cluster is healthy:**

```bash
# All 3 nodes running?
docker compose ps

# Find the leader (one returns 200, the others 503)
curl -s -o /dev/null -w 'node1: %{http_code}\n' localhost:18081/primary
curl -s -o /dev/null -w 'node2: %{http_code}\n' localhost:18082/primary
curl -s -o /dev/null -w 'node3: %{http_code}\n' localhost:18083/primary

# Write and read through HAProxy (always routes to the leader)
curl -s -o /dev/null -w 'PUT: %{http_code}\n' -X PUT localhost:8080/kv/hello -d world
curl localhost:8080/kv/hello
```

**See it survive a leader crash:**

```bash
docker compose kill -s KILL node1   # replace with whichever node is leader
sleep 5
curl -s -o /dev/null -w 'PUT: %{http_code}\n' -X PUT localhost:8080/kv/post-failover -d alive
curl localhost:8080/kv/post-failover
# alive — client never had to know anything changed
```

`compose/smoke-test.sh` automates this entire walkthrough including failover.

## API

| Method   | Path        |                                                  |
| -------- | ----------- | ------------------------------------------------ |
| `PUT`    | `/kv/{key}` | Write, body = raw value bytes. `204` on success. |
| `GET`    | `/kv/{key}` | Read. `200` + raw body, or `404`.                |
| `DELETE` | `/kv/{key}` | Delete. `204` — idempotent.                      |
| `GET`    | `/primary`  | `200` on leader, else `503`.                     |
| `GET`    | `/replica`  | `200` on follower, else `503`.                   |
| `GET`    | `/status`   | `{leader, members, learners, committed_index}`.  |

**`GET ?level=`** picks read consistency:

| `level`             | Meaning                                                                                  |
| ------------------- | ---------------------------------------------------------------------------------------- |
| absent / `eventual` | Local read, fast, may be briefly stale. Default.                                         |
| `linearizable`      | Confirmed via Raft — always current, slower. Requires the leader.                        |
| `lease`             | Leader-lease optimized — as current as linearizable, but faster when the lease is valid. |

## Errors

Every non-2xx response is JSON: `{"error":"...","leader_hint":{...},"trace_id":"..."}`.
`leader_hint` and `trace_id` are included only when relevant; extractor-level
rejections are normalized into the same shape.

- `400` = client error
- `503` = not leader / cluster unavailable, retry
- `500` = server error; check `trace_id` in logs

## Config (`config.toml`, see `config.example.toml`)

```toml
[cluster]
node_id = 1
listen_address = "0.0.0.0:9081"
db_root_dir = "/data/raft"
log_dir = "/data/logs"
# initial_cluster = [...]   # required for multi-node; see compose/node1/config.toml

[lmdb]
data_dir = "/data/lmdb"
map_size_gb = 10

[http]
listen_address = "0.0.0.0:8080"
```

Path via `d-lmdb-server serve --config <path>`, or the `CONFIG` env var
(what Docker uses). `d-lmdb-server healthcheck` probes `/status`.

## High Availability

Put a load balancer in front of an odd number of nodes (3+). Route
`PUT`/`DELETE` and `GET ?level=linearizable|lease` to whichever node's
`/primary` returns `200`. Plain `GET` can be round-robined across live
nodes because reads stay local by default. `compose/haproxy.cfg` is a
working reference; `compose/smoke-test.sh` proves the failover story.

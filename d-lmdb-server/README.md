# d-lmdb-server

---

d-lmdb-server is for developer who want distributed, LMDB-backed key-value
storage as a standalone service — no Rust, no embedding, no client library.
It wraps [d-lmdb](../d-lmdb) behind HTTP+JSON: writes go through Raft
consensus and are replicated across nodes, reads stay local by default.
Docker-deployable — talk to it over plain HTTP from any language.

## Quick Start

**Single node:**

```bash
docker pull deventlab/d-lmdb:latest
cp config.example.toml config.toml
docker run -d -p 8080:8080 -e CONFIG=/etc/dlmdb/config.toml \
  -v $(pwd)/config.toml:/etc/dlmdb/config.toml:ro -v dlmdb-data:/data \
  deventlab/d-lmdb:latest

sleep 3
curl -X PUT localhost:8080/kv/hello -d world
curl localhost:8080/kv/hello
```

**3-node HA cluster** (from repo root):

```bash
docker pull deventlab/d-lmdb:latest
docker tag deventlab/d-lmdb:latest d-lmdb-server:compose
docker compose down -v 2>/dev/null   # clean up any previous run
docker compose up -d
sleep 10   # wait for leader election + HAProxy health checks
curl -X PUT localhost:8080/kv/hello -d world              # via HAProxy, always finds the leader
curl "localhost:8080/kv/hello?level=linearizable"
```

`compose/smoke-test.sh` runs a full automated walkthrough, including killing the leader container and confirming the cluster recovers with zero client-side changes.

## API

| Method   | Path        |                                                                                |
| -------- | ----------- | ------------------------------------------------------------------------------ |
| `PUT`    | `/kv/{key}` | Write, body = raw value bytes. `204` on success.                               |
| `GET`    | `/kv/{key}` | Read. `200` + raw body, or `404`.                                              |
| `DELETE` | `/kv/{key}` | Delete. `204` — idempotent, deleting a missing key still succeeds.             |
| `GET`    | `/primary`  | `200` if this node is leader, else `503`. For LB health checks routing writes. |
| `GET`    | `/replica`  | `200` if this node is a follower, else `503`.                                  |
| `GET`    | `/status`   | Always `200`. `{leader, members, learners, committed_index}`.                  |

**`GET ?level=`** picks read consistency:

| `level`             | Meaning                                                                                                     |
| ------------------- | ----------------------------------------------------------------------------------------------------------- |
| absent / `eventual` | Local read, fast, may be briefly stale. Default.                                                            |
| `linearizable`      | Confirmed via Raft ReadIndex — always current, slower. Requires the leader.                                 |
| `lease`             | Leader-lease optimized — as current as `linearizable`, faster when the lease is valid. Requires the leader. |

## Errors

Every non-2xx response is JSON: `{"error": "...", "leader_hint": {...}, "trace_id": "..."}` (`leader_hint`/`trace_id` only present when relevant — extractor-level rejections get normalized into this shape too, never plain text). `400` = your input, `503` = not leader / cluster unavailable, retry; `500` = server-side, look up `trace_id` in server logs for detail. Full classification and rationale: `decisions/015-http-error-classification.md` in `d-engine-product-design`.

## Config (`config.toml`, see `config.example.toml`)

```toml
[cluster]
node_id = 1
listen_address = "0.0.0.0:9081"
db_root_dir = "/data/raft"
log_dir = "/data/logs"
# initial_cluster = [...]   # required for multi-node — see compose/node1/config.toml

[lmdb]
data_dir = "/data/lmdb"
map_size_gb = 10

[http]
listen_address = "0.0.0.0:8080"
```

Path via `d-lmdb-server serve --config <path>`, or the `CONFIG` env var (what Docker uses). `d-lmdb-server healthcheck` probes `/status` on the same config — this is what the image's `HEALTHCHECK` calls.

## High Availability

Put a load balancer in front of an odd number of nodes (3+), routing `PUT`/`DELETE` and `GET ?level=linearizable|lease` to whichever node's `/primary` returns `200`; plain `GET` can round-robin any live node. `compose/haproxy.cfg` is a working reference, `compose/smoke-test.sh` proves the failover story end to end.

# d-lmdb

![status](https://img.shields.io/badge/status-experimental-orange)
[![CI](https://github.com/deventlab/d-lmdb/actions/workflows/ci.yml/badge.svg)](https://github.com/deventlab/d-lmdb/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/deventlab/d-lmdb/graph/badge.svg)](https://codecov.io/gh/deventlab/d-lmdb)
![Static Badge](https://img.shields.io/badge/license-MIT%20%7C%20Apache--2.0-blue)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/deventlab/d-lmdb)

> Built to demonstrate d-engine's replication capabilities.

## What This Is

d-lmdb is an implementation that demonstrates one concrete way [d-engine](https://github.com/DEventLab/d-engine) can add distributed replication to an existing embedded storage engine. LMDB was chosen because it is a well-known, compact embedded key-value store.

The broader point this project is meant to demonstrate: **d-engine can be paired with many kinds of storage engines and applications to add an optional, replicated write path** — LMDB is just the example chosen here.

d-lmdb wraps LMDB with Raft consensus. Reads stay local and synchronous. Writes go through Raft and become replicated and strongly consistent. The only dependencies beyond d-engine are LMDB and the small set of crates d-engine itself needs.

d-lmdb aims at distributed _fault tolerance_, not distributed _scaling_. It adds a replicated write path in front of the LMDB you already know — the read path is left untouched. See [What d-lmdb Doesn't Solve](#what-d-lmdb-doesnt-solve) for where this stops being useful.

---

## What d-lmdb Solves

- **Single point of failure → quorum fault tolerance.** Writes go through Raft; the cluster can keep serving as long as a majority of nodes are up. Plain LMDB has no replica — a lost disk or crashed process can mean lost data.
- **Cross-machine strong consistency, one additional dependency.** Multiple nodes see the same linearizable write history, with d-engine as the only extra piece — no separate coordination service to run.
- **LMDB's read path stays untouched.** `get()` bypasses Raft entirely and reads the local LMDB file directly — reads are not slowed down by the replication layer.

## What d-lmdb Doesn't Solve

Stating these plainly up front, so expectations are set correctly:

- **Write throughput does not improve — it gets worse.** A local LMDB write is one fsync. A d-lmdb write is a network round trip plus a quorum of fsyncs. This trades write latency and throughput for availability and consistency.
- **No sharding.** d-engine replicates data, it does not partition it. Every node holds the full dataset — if your data does not fit on one node's LMDB, d-lmdb does not help.
- **Still just a key-value store.** Adding Raft does not add relational modeling or complex queries — the surface is the same get/put/scan LMDB already had.
- **Raft adds real operational surface.** Leader election, membership changes, snapshots, log compaction — none of this exists with plain LMDB. Reads keeping their performance does not mean operations stay as simple as LMDB alone.

---

## Migration from LMDB

Most read code doesn't change:

```rust
// LMDB (before)
let val = txn.get(db, &key)?;

// d-lmdb (after)
let val = db.get(&key)?;   // still sync, still local
```

The one change that cannot be avoided: writes are async. d-lmdb must run inside a Tokio async runtime, because
committing a write means coordinating with other nodes through Raft before returning to the caller.
Reads are synchronous and can be called from any context.

```rust
// d-lmdb writes
db.put(b"user:1", b"alice").await?;
db.delete(b"user:1").await?;
```

## Read Consistency

d-lmdb exposes the tradeoff explicitly:

| Method                  | Consistency  | When to use                                                   |
| ----------------------- | ------------ | ------------------------------------------------------------- |
| `get(key)`              | Eventual     | Default — local read, may lag the leader by replication delay |
| `get_linearizable(key)` | Linearizable | When you need read-your-writes after a concurrent write       |

## API

```rust
// Open a node (single-node or cluster, depending on config)
let db = DLmdb::open(DLmdbConfig::new("./data")).await?;
let db = DLmdb::open_from_file("config.toml").await?;   // multi-node: config declares peers
db.wait_ready(Duration::from_secs(5)).await?;             // block until a leader is elected

// Reads — sync, no await, bypass Raft entirely
let val  = db.get(b"user:1")?;
let ok   = db.exists(b"user:1")?;
let n    = db.len();
let many = db.get_multi(&[b"k1", b"k2"])?;

// Scans — sync, bounded
let all  = db.scan_all(Some(1000))?;
let page = db.scan_prefix(b"user:", None, Some(100))?;
let next = db.scan_prefix(b"user:", Some(last_key), Some(100))?;  // pagination
let rng  = db.scan_range(b"user:2024:", b"user:2025:", Some(500))?;
let rev  = db.scan_range_rev(b"log:", b"log:~", Some(20))?;       // last 20, descending

// Linearizable read — async, one Raft round trip
let val = db.get_linearizable(b"key").await?;

// Writes — async, go through Raft
db.put(b"key", b"value").await?;
db.put_with_ttl(b"session:1", b"token", 3600).await?;      // expires after ttl_secs
db.delete(b"key").await?;
db.put_if_absent(b"lock", b"owner").await?;                 // returns bool
db.compare_and_swap(b"k", Some(b"old"), b"new").await?;     // returns bool
db.batch(vec![
    BatchOp::Insert { key: b"a".to_vec(), value: b"1".to_vec() },
    BatchOp::Delete { key: b"b".to_vec() },
]).await?;

// Cluster / lifecycle
db.leader();           // Option<LeaderInfo>
db.is_leader();        // bool
db.cluster_info();     // MembershipSnapshot
db.close().await?;     // graceful shutdown
```

## Architecture

d-engine runs **embedded** — no separate server process. Reads bypass consensus entirely and go directly to the local LMDB instance.

```txt
Your Process
    │
    ├─ db.get(key)  ─────────────────────────► local LMDB  (sync, local read)
    │
    └─ db.put(key, val) ──► d-engine Raft ──► local LMDB  (async, replicated)
                             leader election
                             log replication
                             quorum commit
```

Both the Raft WAL and the KV state machine use LMDB, in two independent environments:

```txt
data_dir/
├── raft/   ← LMDB env for Raft log + hard state
└── lmdb/   ← LMDB env for user KV data
```

## Features

- Strongly consistent writes via Raft — no split-brain
- Sync reads from local LMDB — no network round trip
- Linearizable reads available on demand via `get_linearizable`
- Single storage engine: LMDB only, for both Raft log and KV data
- Embedded mode — runs inside your process, no separate server to operate

## Project Status

d-lmdb is early-stage and evolving.

The core functionality is implemented and intended for evaluation, experimentation, and community feedback.

It has not yet been extensively validated in production environments, so APIs and internal behavior may evolve before a stable release.

Issues, discussions, forks, and pull requests are highly welcome.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.

This project is built on [d-engine](https://github.com/DEventLab/d-engine) (Apache-2.0 / MIT)
and [LMDB](https://www.symas.com/lmdb) (OpenLDAP Public License). See [NOTICES](NOTICES) for third-party attributions.

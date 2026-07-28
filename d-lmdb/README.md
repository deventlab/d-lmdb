# d-lmdb

---

d-lmdb is for Rust developers who want LMDB as their storage engine, with
distributed fault tolerance built in. It replicates LMDB across nodes (via
Raft consensus): reads stay local and synchronous, writes are replicated and
strongly consistent.

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

## API

```rust
// Open a node
let db = DLmdb::open("./data").await?;                   // single-node: just a data directory
let db = DLmdb::open_from_file("config.toml").await?;    // multi-node: config declares peers
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

## Read Consistency

d-lmdb exposes the tradeoff explicitly:

| Method                  | Consistency  | When to use                                                   |
| ----------------------- | ------------ | ------------------------------------------------------------- |
| `get(key)`              | Eventual     | Default — local read, may lag the leader by replication delay |
| `get_linearizable(key)` | Linearizable | When you need read-your-writes after a concurrent write       |

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

## Failure Behavior

- If no leader is elected yet, writes block until `wait_ready()` completes or timeout.
- If the leader is unreachable, `put()`/`delete()` return an error — the caller must retry.
- Reads never fail due to leader unavailability (they bypass Raft entirely).

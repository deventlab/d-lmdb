//! Three-node cluster example.
//!
//! Run three nodes in separate processes, each with its own config file:
//!
//!   # terminal 1
//!   CONFIG=examples/three-node/config/n1.toml cargo run --example three_node
//!
//!   # terminal 2
//!   CONFIG=examples/three-node/config/n2.toml cargo run --example three_node
//!
//!   # terminal 3
//!   CONFIG=examples/three-node/config/n3.toml cargo run --example three_node
//!
//! Once all three are up, writes on any node replicate to the others.
//! Killing one node leaves the cluster operational (quorum = 2 of 3).

use std::time::Duration;

use bytes::Bytes;
use d_lmdb::BatchOp;
use d_lmdb::DLmdb;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config_path = std::env::var("CONFIG")
        .unwrap_or_else(|_| "examples/three-node/config/n1.toml".to_string());

    // Multi-node: config file required (declares node_id, peers, listen_address).
    // wait_ready blocks until Raft elects a leader — mandatory before writes.
    let db = DLmdb::open_from_file(&config_path).await?;
    db.wait_ready(Duration::from_secs(10)).await?;

    // ── Cluster info ──────────────────────────────────────────────────────────
    if let Some(leader) = db.leader() {
        println!("leader = node:{}", leader.leader_id);
    }
    let info = db.cluster_info();
    println!("cluster members = {}", info.members.len());

    // ── Basic KV — same API as single-node ───────────────────────────────────
    if let Err(e) = db.put(b"user:1", b"alice").await {
        eprintln!("db put error: {e:?}");
    }

    if let Some(v) = db.get(b"user:1")? {
        println!("user:1 = {}", String::from_utf8_lossy(&v));
    }

    // ── Batch — atomic across all replicas ───────────────────────────────────
    // One Raft entry: all ops replicate atomically to every node.
    if let Err(e) = db
        .batch(vec![
            BatchOp::Insert {
                key: Bytes::from_static(b"user:1"),
                value: Bytes::from_static(b"alice_v2"),
            },
            BatchOp::Insert {
                key: Bytes::from_static(b"idx:name:alice_v2"),
                value: Bytes::from_static(b"1"),
            },
            BatchOp::Delete {
                key: Bytes::from_static(b"idx:name:alice"),
            },
        ])
        .await
    {
        eprintln!("db batch error: {e:?}");
    }

    // ── Read-modify-write — CAS loop ──────────────────────────────────────────
    // In a multi-node cluster, concurrent writers on different nodes can race.
    // CAS detects the conflict; the retry loop converges in O(contention) rounds.
    if let Err(e) = db.put(b"counter:views", b"0").await {
        eprintln!("db put error: {e:?}");
    }

    loop {
        let current = match db.get_linearizable(b"counter:views").await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("db get_linearizable error: {e:?}");
                break;
            }
        };
        let next = current
            .as_deref()
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(|s| s.parse::<u64>().ok())
            .map(|n| (n + 1).to_string())
            .unwrap_or_else(|| "1".to_string());

        match db.compare_and_swap(b"counter:views", current.as_deref(), next.as_bytes()).await {
            Ok(success) => {
                if success {
                    println!("counter:views = {next}");
                    break;
                } else {
                    continue;
                }
            }
            Err(e) => {
                eprintln!("db CAS error: {e:?}");
                break;
            }
        }
    }

    // ── Linearizable read — cross-node consistency ────────────────────────────
    // After a write on node 1, node 2 or 3 may not yet have applied it.
    // get_linearizable() guarantees the read reflects all committed writes.
    match db.get_linearizable(b"counter:views").await {
        Ok(views) => {
            if let Some(v) = views {
                println!(
                    "linearizable counter:views = {}",
                    String::from_utf8_lossy(&v)
                );
            }
        }
        Err(e) => eprintln!("db get_linearizable error: {e:?}"),
    }

    // ── TTL ───────────────────────────────────────────────────────────────────
    // TTL is enforced cluster-wide: expiry is based on wall-clock at write time,
    // replicated as part of the Raft entry, consistent on all nodes.
    if let Err(e) = db.put_with_ttl(b"session:tok_xyz", b"user:1", 30).await {
        eprintln!("db put_with_ttl error: {e:?}");
    }

    if let Err(e) = db.put_with_ttl(b"lock:report:7", b"node:1", 10).await {
        eprintln!("db put_with_ttl error: {e:?}");
    }

    // ── Scans ─────────────────────────────────────────────────────────────────
    let users = db.scan_prefix(b"user:", None, Some(100))?;
    println!("users on this node: {}", users.entries.len());

    // ── Keep node alive — wait for Ctrl+C ────────────────────────────────────
    println!("Node running. Press Ctrl+C to shutdown gracefully.");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("Received Ctrl+C, shutting down gracefully...");
        }
    }

    db.close().await?;
    Ok(())
}

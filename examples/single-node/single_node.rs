//! Single-node d-lmdb example.
//!
//! Zero config: just supply a data directory.
//! No config.toml, no wait_ready — open and use immediately.
//!
//! Usage:
//!   cargo run --example single_node

use std::time::Duration;

use bytes::Bytes;
use d_lmdb::BatchOp;
use d_lmdb::DLmdb;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    // Zero config: single node needs only a data directory.
    // No config.toml, no cluster setup, no wait_ready.
    let db = DLmdb::open("./data").await?;
    db.wait_ready(Duration::from_secs(10)).await?;

    // ── 1. Basic put / get / delete ───────────────────────────────────────────
    db.put(b"user:1", b"alice").await?;
    db.put(b"user:2", b"bob").await?;

    // get returns Option<Vec<u8>> — no extra imports needed
    if let Some(v) = db.get(b"user:1")? {
        println!("user:1 = {}", String::from_utf8_lossy(&v));
    }

    assert!(db.exists(b"user:2")?);

    db.delete(b"user:2").await?;
    assert!(!db.exists(b"user:2")?);

    // ── 2. Batch — atomic multi-key write ─────────────────────────────────────
    // Replaces LMDB's rw transaction for multi-key atomicity.
    // All ops land in one Raft entry: either all commit or none do.
    db.batch(vec![
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
    .await?;

    if let Some(v) = db.get(b"user:1")? {
        println!("after batch, user:1 = {}", String::from_utf8_lossy(&v));
    }

    // ── 3. Read-modify-write — CAS loop ───────────────────────────────────────
    // LMDB equivalent:
    //   let mut txn = env.begin_rw_txn()?;
    //   let v = txn.get(db, b"counter")?;
    //   txn.put(db, b"counter", new_val)?;
    //   txn.commit()?;
    //
    // In d-lmdb, writes go through Raft consensus so there is no single-node
    // rw transaction. Use a CAS retry loop instead. In practice, contention on
    // a single key is rare — one retry suffices under normal load.
    db.put(b"counter:views", b"1000").await?;
    loop {
        let current = db.get(b"counter:views")?;
        let next = current
            .as_deref()
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(|s| s.parse::<u64>().ok())
            .map(|n| (n + 1).to_string())
            .unwrap_or_else(|| "1".to_string());

        if db
            .compare_and_swap(b"counter:views", current.as_deref(), next.as_bytes())
            .await?
        {
            println!("counter:views incremented to {next}");
            break;
        }
        // lost the race — retry
    }

    // ── 4. TTL — expiring keys ────────────────────────────────────────────────
    // Session token expires in 30 s; get() returns None after expiry.
    db.put_with_ttl(b"session:tok_abc123", b"user:1", 30).await?;

    // Distributed advisory lock with 10 s lease.
    let acquired = db.put_if_absent(b"lock:invoice:42", b"node:1").await?;
    println!("lock acquired = {acquired}");

    db.put_with_ttl(b"cache:product:99", br#"{"price":9.99}"#, 300).await?;

    // ── 5. Scans ──────────────────────────────────────────────────────────────
    db.put(b"user:3", b"carol").await?;

    let users = db.scan_prefix(b"user:", None, Some(100))?;
    println!("users ({}):", users.entries.len());
    for (k, v) in &users.entries {
        println!(
            "  {} = {}",
            String::from_utf8_lossy(k),
            String::from_utf8_lossy(v)
        );
    }

    // Paginated scan: pass last key of previous page as `after`.
    let page1 = db.scan_prefix(b"user:", None, Some(2))?;
    if let Some((last_key, _)) = page1.entries.last() {
        let page2 = db.scan_prefix(b"user:", Some(last_key), Some(2))?;
        println!("page2 count = {}", page2.entries.len());
    }

    // Range scan [start, end)
    let range = db.scan_range(b"user:1", b"user:9", Some(10))?;
    println!("range scan count = {}", range.entries.len());

    // ── 6. Linearizable read — read-your-writes guarantee ────────────────────
    // Use when you must reflect the latest committed write (e.g. after a put
    // on another node). Costs one Raft ReadIndex round-trip.
    let views = db.get_linearizable(b"counter:views").await?;
    if let Some(v) = views {
        println!(
            "linearizable counter:views = {}",
            String::from_utf8_lossy(&v)
        );
    }

    // ── Cleanup ───────────────────────────────────────────────────────────────
    db.close().await?;
    Ok(())
}

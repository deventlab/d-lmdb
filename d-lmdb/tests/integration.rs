use std::time::Duration;

use d_lmdb::DLmdb;

async fn open_single_node(dir: &std::path::Path) -> d_lmdb::Result<DLmdb> {
    let db = DLmdb::open(dir).await?;
    db.wait_ready(Duration::from_secs(10)).await?;
    Ok(db)
}

#[tokio::test]
async fn put_and_get_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_single_node(dir.path()).await.unwrap();

    db.put(b"hello", b"world").await.unwrap();

    let val = db.get(b"hello").unwrap();
    assert_eq!(val.as_deref(), Some(b"world".as_ref()));

    db.close().await.unwrap();
}

#[tokio::test]
async fn delete_removes_key() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_single_node(dir.path()).await.unwrap();

    db.put(b"key", b"value").await.unwrap();
    db.delete(b"key").await.unwrap();

    let val = db.get(b"key").unwrap();
    assert_eq!(val, None);

    db.close().await.unwrap();
}

#[tokio::test]
async fn compare_and_swap_succeeds_on_match() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_single_node(dir.path()).await.unwrap();

    db.put(b"k", b"v1").await.unwrap();

    let swapped = db.compare_and_swap(b"k", Some(b"v1"), b"v2").await.unwrap();
    assert!(swapped);

    let val = db.get(b"k").unwrap();
    assert_eq!(val.as_deref(), Some(b"v2".as_ref()));

    db.close().await.unwrap();
}

#[tokio::test]
async fn compare_and_swap_fails_on_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_single_node(dir.path()).await.unwrap();

    db.put(b"k", b"v1").await.unwrap();

    let swapped = db.compare_and_swap(b"k", Some(b"wrong"), b"v2").await.unwrap();
    assert!(!swapped);

    let val = db.get(b"k").unwrap();
    assert_eq!(val.as_deref(), Some(b"v1".as_ref())); // unchanged

    db.close().await.unwrap();
}

#[tokio::test]
async fn rejects_oversized_key() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_single_node(dir.path()).await.unwrap();

    let big_key = vec![b'k'; 513];
    let result = db.put(&big_key, b"v").await;
    assert!(matches!(result, Err(d_lmdb::Error::KeyTooLarge(_))));

    db.close().await.unwrap();
}

#[tokio::test]
async fn rejects_oversized_value() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_single_node(dir.path()).await.unwrap();

    let big_value = vec![0u8; 1024 * 1024 + 1];
    let result = db.put(b"k", &big_value).await;
    assert!(matches!(result, Err(d_lmdb::Error::ValueTooLarge(_))));

    db.close().await.unwrap();
}

// ── TTL semantics (owned entirely by db.rs — LmdbStateMachine never sees TTL) ──

#[tokio::test]
async fn put_with_ttl_returns_value_before_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_single_node(dir.path()).await.unwrap();

    db.put_with_ttl(b"session:1", b"user_data", 3600).await.unwrap();

    let val = db.get(b"session:1").unwrap();
    assert_eq!(val.as_deref(), Some(b"user_data".as_ref()));

    db.close().await.unwrap();
}

#[tokio::test]
async fn put_with_ttl_expires_and_is_physically_removed() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_single_node(dir.path()).await.unwrap();

    // ttl_secs = 0 -> expires_at is computed as "now" at write time, already
    // expired by the time get() runs a moment later. No sleep needed.
    db.put_with_ttl(b"session:1", b"user_data", 0).await.unwrap();

    let val = db.get(b"session:1").unwrap();
    assert_eq!(val, None);

    // get() must have triggered delete_local — confirm the key is physically
    // gone (len() == 0), not just logically expired but still sitting in LMDB.
    assert!(!db.exists(b"session:1").unwrap());
    assert_eq!(db.len(), 0);

    db.close().await.unwrap();
}

#[tokio::test]
async fn scan_all_excludes_expired_ttl_keys() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_single_node(dir.path()).await.unwrap();

    db.put(b"alive", b"ok").await.unwrap();
    db.put_with_ttl(b"dead", b"nope", 0).await.unwrap();

    let scan = db.scan_all(None).unwrap();
    let keys: Vec<&[u8]> = scan.entries.iter().map(|(k, _)| k.as_ref()).collect();
    assert!(keys.contains(&b"alive".as_ref()), "active key must appear");
    assert!(
        !keys.contains(&b"dead".as_ref()),
        "expired key must be excluded"
    );

    db.close().await.unwrap();
}

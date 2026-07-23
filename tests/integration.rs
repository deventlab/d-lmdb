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

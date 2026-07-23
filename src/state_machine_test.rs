use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use d_engine::state_machine_test::StateMachineBuilder;
use d_engine::state_machine_test::StateMachineTestSuite;
use d_engine::ApplyEntry;
use d_engine::Command;
use d_engine::Error as EngineError;
use d_engine::StateMachine;
use tempfile::TempDir;

use crate::config::DLmdbConfig;
use crate::state_machine::LmdbStateMachine;
use crate::wire::Value;

struct LmdbStateMachineBuilder {
    temp_dir: TempDir,
}

impl LmdbStateMachineBuilder {
    fn new() -> Self {
        Self {
            temp_dir: TempDir::new().expect("temp dir"),
        }
    }
}

#[async_trait]
impl StateMachineBuilder for LmdbStateMachineBuilder {
    async fn build(&self) -> Result<Arc<dyn StateMachine>, EngineError> {
        let path = self.temp_dir.path().join("lmdb_sm");
        let config = DLmdbConfig::new(self.temp_dir.path());
        let sm = LmdbStateMachine::new(path, &config).await?;
        Ok(Arc::new(sm))
    }

    async fn cleanup(&self) -> Result<(), EngineError> {
        Ok(())
    }
}

#[tokio::test]
async fn test_lmdb_state_machine_suite() {
    let builder = LmdbStateMachineBuilder::new();
    StateMachineTestSuite::run_all_tests(builder)
        .await
        .expect("LmdbStateMachine must pass all tests");
}

// ── TTL decode correctness tests ─────────────────────────────────────────────
//
// These tests bypass Raft and drive LmdbStateMachine directly via apply_chunk.
// Values are pre-encoded with Value::encode_raw / encode_ttl to match exactly
// what db.rs writes into the Raft log. This verifies that get() decodes the
// tag-prefixed wire format and enforces TTL expiry correctly.

async fn make_sm() -> (LmdbStateMachine, TempDir) {
    let tmp = TempDir::new().expect("temp dir");
    let config = DLmdbConfig::new(tmp.path());
    let sm = LmdbStateMachine::new(tmp.path().join("lmdb"), &config)
        .await
        .expect("create state machine");
    (sm, tmp)
}

fn insert_entry(
    index: u64,
    key: &[u8],
    encoded_value: Vec<u8>,
) -> ApplyEntry {
    ApplyEntry {
        index,
        term: 1,
        command: Command::Insert {
            key: Bytes::copy_from_slice(key),
            value: Bytes::from(encoded_value),
            ttl_secs: None,
        },
    }
}

#[tokio::test]
async fn test_get_raw_encoded_value_returns_clean_payload() {
    // A value stored via encode_raw must be returned without the 0x00 tag byte.
    let (sm, _tmp) = make_sm().await;
    let encoded = Value::encode_raw(b"alice");
    sm.apply_chunk(&[insert_entry(1, b"user:1", encoded)]).await.unwrap();

    let result = sm.get(b"user:1").unwrap();
    assert_eq!(result, Some(Bytes::from_static(b"alice")));
}

#[tokio::test]
async fn test_get_active_ttl_value_returns_clean_payload() {
    // A TTL value that has not expired must return only the payload, not the
    // [0x01][expires_at][payload] wire bytes.
    let (sm, _tmp) = make_sm().await;
    let encoded = Value::encode_ttl(b"session_data", Some(u64::MAX));
    sm.apply_chunk(&[insert_entry(1, b"session:1", encoded)]).await.unwrap();

    let result = sm.get(b"session:1").unwrap();
    assert_eq!(result, Some(Bytes::from_static(b"session_data")));
}

#[tokio::test]
async fn test_get_expired_ttl_value_returns_none() {
    // A TTL value with expires_at = 0 (Unix epoch, always in the past) must
    // be treated as expired: get() must return None.
    let (sm, _tmp) = make_sm().await;
    let encoded = Value::encode_ttl(b"stale", Some(0));
    sm.apply_chunk(&[insert_entry(1, b"expired:key", encoded)]).await.unwrap();

    let result = sm.get(b"expired:key").unwrap();
    assert_eq!(result, None);
}

#[tokio::test]
async fn test_get_expired_ttl_lazily_deletes_key() {
    // After get() on an expired key, the key must be physically removed from
    // LMDB so that subsequent reads don't hit disk unnecessarily.
    let (sm, _tmp) = make_sm().await;
    let encoded = Value::encode_ttl(b"gone", Some(0));
    sm.apply_chunk(&[insert_entry(1, b"lazy:del", encoded)]).await.unwrap();

    // First get triggers lazy delete.
    let _ = sm.get(b"lazy:del").unwrap();

    // Second get must also return None — key is physically gone from LMDB.
    let result = sm.get(b"lazy:del").unwrap();
    assert_eq!(result, None);

    // scan_all must not include the deleted key.
    let scan = sm.scan_all(None).unwrap();
    let found = scan.entries.iter().any(|(k, _)| k.as_ref() == b"lazy:del");
    assert!(
        !found,
        "expired key must not appear in scan after lazy delete"
    );
}

#[tokio::test]
async fn test_scan_excludes_expired_ttl_keys() {
    // Expired keys must be silently skipped in scan results.
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[
        insert_entry(1, b"alive", Value::encode_ttl(b"ok", Some(u64::MAX))),
        insert_entry(2, b"dead", Value::encode_ttl(b"nope", Some(0))),
    ])
    .await
    .unwrap();

    let scan = sm.scan_all(None).unwrap();
    let keys: Vec<&[u8]> = scan.entries.iter().map(|(k, _)| k.as_ref()).collect();
    assert!(keys.contains(&b"alive".as_ref()), "active key must appear");
    assert!(
        !keys.contains(&b"dead".as_ref()),
        "expired key must be excluded"
    );
}

#[tokio::test]
async fn test_scan_returns_clean_payload_for_active_ttl_key() {
    // Active TTL keys in scan results must carry only the payload — not the
    // [0x01][expires_at][payload] wire bytes.
    let (sm, _tmp) = make_sm().await;
    let encoded = Value::encode_ttl(b"value", Some(u64::MAX));
    sm.apply_chunk(&[insert_entry(1, b"k", encoded)]).await.unwrap();

    let scan = sm.scan_all(None).unwrap();
    assert_eq!(scan.entries.len(), 1);
    assert_eq!(scan.entries[0].1, Bytes::from_static(b"value"));
}

// ── scan_prefix_bounded correctness tests ────────────────────────────────────
//
// Same empty-key guard as scan_range: an empty prefix must not be handed to
// LMDB as a range bound. This path had zero test coverage before, which is
// why the MDB_BAD_VALSIZE gap here went unnoticed.

#[tokio::test]
async fn test_scan_prefix_returns_empty_for_empty_prefix() {
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[insert_entry(1, b"user:1", Value::encode_raw(b"alice"))])
        .await
        .unwrap();

    let scan = sm.scan_prefix_bounded(b"", None, None).unwrap();
    assert_eq!(scan.entries.len(), 0);
}

#[tokio::test]
async fn test_scan_prefix_returns_matching_keys() {
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[
        insert_entry(1, b"user:1", Value::encode_raw(b"alice")),
        insert_entry(2, b"user:2", Value::encode_raw(b"bob")),
        insert_entry(3, b"session:1", Value::encode_raw(b"tok")),
    ])
    .await
    .unwrap();

    let scan = sm.scan_prefix_bounded(b"user:", None, None).unwrap();
    let keys: Vec<&[u8]> = scan.entries.iter().map(|(k, _)| k.as_ref()).collect();
    assert_eq!(keys, vec![b"user:1".as_ref(), b"user:2".as_ref()]);
}

// ── scan_range_rev correctness tests ─────────────────────────────────────────
//
// LMDB rejects zero-length keys as a range bound (MDB_BAD_VALSIZE). scan_range_rev
// guards against an empty `start` by returning an empty result instead of handing
// b"" to LMDB's range API. These tests cover that guard plus the reverse-scan
// ordering/limit/TTL semantics that scan_range_rev is responsible for.

#[tokio::test]
async fn test_scan_range_rev_returns_empty_for_empty_start() {
    // Regression test: an empty `start` must short-circuit to an empty result,
    // not be handed to LMDB as a zero-length range bound.
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[insert_entry(1, b"a", Value::encode_raw(b"1"))]).await.unwrap();

    let scan = sm.scan_range_rev(b"", b"z", None).unwrap();
    assert_eq!(scan.entries.len(), 0);
}

#[tokio::test]
async fn test_scan_range_rev_returns_keys_in_descending_order() {
    // scan_range_rev must return matches in reverse key order, not insertion order.
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[
        insert_entry(1, b"log:1", Value::encode_raw(b"a")),
        insert_entry(2, b"log:2", Value::encode_raw(b"b")),
        insert_entry(3, b"log:3", Value::encode_raw(b"c")),
    ])
    .await
    .unwrap();

    let scan = sm.scan_range_rev(b"log:", b"log:~", None).unwrap();
    let keys: Vec<&[u8]> = scan.entries.iter().map(|(k, _)| k.as_ref()).collect();
    assert_eq!(keys, vec![b"log:3".as_ref(), b"log:2".as_ref(), b"log:1".as_ref()]);
}

#[tokio::test]
async fn test_scan_range_rev_respects_limit() {
    // With a limit smaller than the range size, only the highest-keyed entries
    // (the sliding window's tail) must be returned, still in descending order.
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[
        insert_entry(1, b"log:1", Value::encode_raw(b"a")),
        insert_entry(2, b"log:2", Value::encode_raw(b"b")),
        insert_entry(3, b"log:3", Value::encode_raw(b"c")),
    ])
    .await
    .unwrap();

    let scan = sm.scan_range_rev(b"log:", b"log:~", Some(2)).unwrap();
    let keys: Vec<&[u8]> = scan.entries.iter().map(|(k, _)| k.as_ref()).collect();
    assert_eq!(keys, vec![b"log:3".as_ref(), b"log:2".as_ref()]);
}

#[tokio::test]
async fn test_scan_range_rev_excludes_expired_ttl_keys() {
    // Expired TTL keys must be filtered out of reverse scans, same as forward scans.
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[
        insert_entry(1, b"alive", Value::encode_ttl(b"ok", Some(u64::MAX))),
        insert_entry(2, b"dead", Value::encode_ttl(b"nope", Some(0))),
    ])
    .await
    .unwrap();

    let scan = sm.scan_range_rev(b"a", b"z", None).unwrap();
    let keys: Vec<&[u8]> = scan.entries.iter().map(|(k, _)| k.as_ref()).collect();
    assert!(keys.contains(&b"alive".as_ref()), "active key must appear");
    assert!(!keys.contains(&b"dead".as_ref()), "expired key must be excluded");
}

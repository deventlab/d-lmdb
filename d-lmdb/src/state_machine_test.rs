use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use d_engine::ApplyEntry;
use d_engine::Command;
use d_engine::Error as EngineError;
use d_engine::StateMachine;
use d_engine::state_machine_test::StateMachineBuilder;
use d_engine::state_machine_test::StateMachineTestSuite;
use tempfile::TempDir;

use crate::config::DLmdbConfig;
use crate::state_machine::LmdbStateMachine;

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

// These tests bypass Raft and drive LmdbStateMachine directly via apply_chunk.
// LmdbStateMachine never interprets `value`/TTL semantics itself — that lives in
// db.rs now (see db_test.rs for TTL-specific coverage). Tests here only cover
// mechanism: byte transparency, the empty-key guard, ordering/limit, and the
// scan filter plumbing.

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

fn cas_entry(
    index: u64,
    key: &[u8],
    expected: Option<&[u8]>,
    value: &[u8],
) -> ApplyEntry {
    ApplyEntry {
        index,
        term: 1,
        command: Command::CompareAndSwap {
            key: Bytes::copy_from_slice(key),
            expected: expected.map(Bytes::copy_from_slice),
            value: Bytes::copy_from_slice(value),
        },
    }
}

fn batch_entry(
    index: u64,
    ops: Vec<d_engine::BatchOp>,
) -> ApplyEntry {
    ApplyEntry {
        index,
        term: 1,
        command: Command::Batch { ops },
    }
}

// ── Write-path transparency (M1/M2) ──────────────────────────────────────────
//
// LmdbStateMachine must never interpret `value` bytes for ANY write command —
// Insert, CompareAndSwap, or Batch. This is what lets the generic
// StateMachineTestSuite (which writes raw, unwrapped bytes directly) round-trip
// correctly. These payloads are deliberately NOT wire-tagged and some start
// with bytes (0x00 / 0x01) that the old tag-parsing design would have
// misinterpreted as a TTL/raw marker and silently stripped or rejected.

#[tokio::test]
async fn test_insert_stores_value_byte_for_byte_untouched() {
    let (sm, _tmp) = make_sm().await;
    let raw = vec![0x00, 0x41, 0x42, 0x43];
    sm.apply_chunk(&[insert_entry(1, b"k1", raw.clone())]).await.unwrap();
    let result = sm.get(b"k1").unwrap();
    assert_eq!(result, Some(Bytes::from(raw)));
}

#[tokio::test]
async fn test_compare_and_swap_stores_value_byte_for_byte_untouched() {
    let (sm, _tmp) = make_sm().await;
    let raw = vec![0x01, 0x02, 0x03];
    sm.apply_chunk(&[cas_entry(1, b"k1", None, &raw)]).await.unwrap();
    let result = sm.get(b"k1").unwrap();
    assert_eq!(result, Some(Bytes::from(raw)));
}

#[tokio::test]
async fn test_batch_insert_stores_value_byte_for_byte_untouched() {
    let (sm, _tmp) = make_sm().await;
    let raw = vec![0x00];
    sm.apply_chunk(&[batch_entry(
        1,
        vec![d_engine::BatchOp::Insert {
            key: Bytes::from_static(b"k1"),
            value: Bytes::from(raw.clone()),
        }],
    )])
    .await
    .unwrap();
    let result = sm.get(b"k1").unwrap();
    assert_eq!(result, Some(Bytes::from(raw)));
}

// ── delete_local (M2) ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_delete_local_removes_key() {
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[insert_entry(1, b"k1", b"v1".to_vec())]).await.unwrap();
    assert_eq!(sm.get(b"k1").unwrap(), Some(Bytes::from_static(b"v1")));

    sm.delete_local(b"k1").unwrap();

    assert_eq!(sm.get(b"k1").unwrap(), None);
}

// ── scan filter mechanism (M3) ────────────────────────────────────────────────
//
// LmdbStateMachine doesn't know what TTL is — `filter` is an opaque function
// supplied by the caller. These tests use a toy filter (keep values longer than
// 1 byte, drop the rest) purely to prove the plumbing works: counting/limit
// interact correctly with a caller-supplied decision, and `None` means "no
// filter, return everything verbatim" — what the generic conformance suite
// relies on. TTL-specific filter behavior belongs to db_test.rs.

fn only_long_values(v: &[u8]) -> Option<Vec<u8>> {
    if v.len() > 1 { Some(v.to_vec()) } else { None }
}

#[tokio::test]
async fn test_scan_all_without_filter_returns_values_verbatim() {
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[insert_entry(1, b"k1", vec![0x00, 0x41])]).await.unwrap();

    let scan = sm.scan_all(None, None::<fn(&[u8]) -> Option<Vec<u8>>>).unwrap();
    assert_eq!(scan.entries.len(), 1);
    assert_eq!(scan.entries[0].1, Bytes::from(vec![0x00, 0x41]));
}

#[tokio::test]
async fn test_scan_all_with_filter_excludes_and_transforms() {
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[
        insert_entry(1, b"short", vec![0x01]),
        insert_entry(2, b"long", vec![0x01, 0x02]),
    ])
    .await
    .unwrap();

    let scan = sm.scan_all(None, Some(only_long_values)).unwrap();
    let keys: Vec<&[u8]> = scan.entries.iter().map(|(k, _)| k.as_ref()).collect();
    assert_eq!(keys, vec![b"long".as_ref()]);
}

#[tokio::test]
async fn test_scan_all_with_filter_respects_limit_exactly() {
    // limit must count only entries the filter accepts, in a single pass —
    // not "fetch `limit` raw entries, then filter" (which could under-deliver).
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[
        insert_entry(1, b"k1", vec![0x01]),
        insert_entry(2, b"k2", vec![0x01, 0x02]),
        insert_entry(3, b"k3", vec![0x01]),
        insert_entry(4, b"k4", vec![0x01, 0x02]),
    ])
    .await
    .unwrap();

    let scan = sm.scan_all(Some(2), Some(only_long_values)).unwrap();
    let keys: Vec<&[u8]> = scan.entries.iter().map(|(k, _)| k.as_ref()).collect();
    assert_eq!(keys, vec![b"k2".as_ref(), b"k4".as_ref()]);
}

// ── scan_prefix_bounded correctness tests ────────────────────────────────────
//
// Same empty-key guard as scan_range: an empty prefix must not be handed to
// LMDB as a range bound. This path had zero test coverage before, which is
// why the MDB_BAD_VALSIZE gap here went unnoticed.

#[tokio::test]
async fn test_scan_prefix_returns_empty_for_empty_prefix() {
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[insert_entry(1, b"user:1", b"alice".to_vec())]).await.unwrap();

    let scan = sm
        .scan_prefix_bounded(b"", None, None, None::<fn(&[u8]) -> Option<Vec<u8>>>)
        .unwrap();
    assert_eq!(scan.entries.len(), 0);
}

#[tokio::test]
async fn test_scan_prefix_returns_matching_keys() {
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[
        insert_entry(1, b"user:1", b"alice".to_vec()),
        insert_entry(2, b"user:2", b"bob".to_vec()),
        insert_entry(3, b"session:1", b"tok".to_vec()),
    ])
    .await
    .unwrap();

    let scan = sm
        .scan_prefix_bounded(b"user:", None, None, None::<fn(&[u8]) -> Option<Vec<u8>>>)
        .unwrap();
    let keys: Vec<&[u8]> = scan.entries.iter().map(|(k, _)| k.as_ref()).collect();
    assert_eq!(keys, vec![b"user:1".as_ref(), b"user:2".as_ref()]);
}

// ── scan_range_rev correctness tests ─────────────────────────────────────────
//
// LMDB rejects zero-length keys as a range bound (MDB_BAD_VALSIZE). scan_range_rev
// guards against an empty `start` by returning an empty result instead of handing
// b"" to LMDB's range API.

#[tokio::test]
async fn test_scan_range_rev_returns_empty_for_empty_start() {
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[insert_entry(1, b"a", b"1".to_vec())]).await.unwrap();

    let scan = sm
        .scan_range_rev(b"", b"z", None, None::<fn(&[u8]) -> Option<Vec<u8>>>)
        .unwrap();
    assert_eq!(scan.entries.len(), 0);
}

#[tokio::test]
async fn test_scan_range_rev_returns_keys_in_descending_order() {
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[
        insert_entry(1, b"log:1", b"a".to_vec()),
        insert_entry(2, b"log:2", b"b".to_vec()),
        insert_entry(3, b"log:3", b"c".to_vec()),
    ])
    .await
    .unwrap();

    let scan = sm
        .scan_range_rev(
            b"log:",
            b"log:~",
            None,
            None::<fn(&[u8]) -> Option<Vec<u8>>>,
        )
        .unwrap();
    let keys: Vec<&[u8]> = scan.entries.iter().map(|(k, _)| k.as_ref()).collect();
    assert_eq!(
        keys,
        vec![b"log:3".as_ref(), b"log:2".as_ref(), b"log:1".as_ref()]
    );
}

#[tokio::test]
async fn test_scan_range_rev_respects_limit() {
    let (sm, _tmp) = make_sm().await;
    sm.apply_chunk(&[
        insert_entry(1, b"log:1", b"a".to_vec()),
        insert_entry(2, b"log:2", b"b".to_vec()),
        insert_entry(3, b"log:3", b"c".to_vec()),
    ])
    .await
    .unwrap();

    let scan = sm
        .scan_range_rev(
            b"log:",
            b"log:~",
            Some(2),
            None::<fn(&[u8]) -> Option<Vec<u8>>>,
        )
        .unwrap();
    let keys: Vec<&[u8]> = scan.entries.iter().map(|(k, _)| k.as_ref()).collect();
    assert_eq!(keys, vec![b"log:3".as_ref(), b"log:2".as_ref()]);
}

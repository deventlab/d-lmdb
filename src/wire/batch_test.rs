use super::{decode, encode};
use crate::BatchOp;

// ── encode + decode round-trips ──────────────────────────────────────────────

#[test]
fn test_roundtrip_put_op() {
    // A single Put op must survive encode → decode intact.
    let ops = vec![BatchOp::put(b"key", b"value")];
    let encoded = encode(&ops).unwrap();
    let decoded = decode(&encoded).unwrap();
    assert_eq!(decoded.len(), 1);
    match &decoded[0] {
        BatchOp::Put { key, value } => {
            assert_eq!(key, b"key");
            assert_eq!(value, b"value");
        }
        BatchOp::Delete { .. } => panic!("expected Put"),
    }
}

#[test]
fn test_roundtrip_delete_op() {
    // A single Delete op must survive encode → decode intact.
    let ops = vec![BatchOp::delete(b"del-key")];
    let encoded = encode(&ops).unwrap();
    let decoded = decode(&encoded).unwrap();
    assert_eq!(decoded.len(), 1);
    match &decoded[0] {
        BatchOp::Delete { key } => assert_eq!(key, b"del-key"),
        BatchOp::Put { .. } => panic!("expected Delete"),
    }
}

#[test]
fn test_roundtrip_empty_batch() {
    // An empty batch must encode and decode without error.
    let ops: Vec<BatchOp> = vec![];
    let encoded = encode(&ops).unwrap();
    let decoded = decode(&encoded).unwrap();
    assert!(decoded.is_empty());
}

#[test]
fn test_roundtrip_mixed_batch_preserves_order() {
    // Mixed Put+Delete must preserve insertion order after round-trip.
    let ops = vec![
        BatchOp::put(b"k1", b"v1"),
        BatchOp::delete(b"k2"),
        BatchOp::put(b"k3", b"v3"),
    ];
    let encoded = encode(&ops).unwrap();
    let decoded = decode(&encoded).unwrap();
    assert_eq!(decoded.len(), 3);

    match &decoded[0] {
        BatchOp::Put { key, value } => {
            assert_eq!(key, b"k1");
            assert_eq!(value, b"v1");
        }
        _ => panic!("expected Put at index 0"),
    }
    match &decoded[1] {
        BatchOp::Delete { key } => assert_eq!(key, b"k2"),
        _ => panic!("expected Delete at index 1"),
    }
    match &decoded[2] {
        BatchOp::Put { key, value } => {
            assert_eq!(key, b"k3");
            assert_eq!(value, b"v3");
        }
        _ => panic!("expected Put at index 2"),
    }
}

// ── decode error handling ─────────────────────────────────────────────────────

#[test]
fn test_decode_garbage_bytes_returns_error() {
    // Corrupted or random bytes must not decode silently.
    let garbage = b"\xFF\xFE\xFD\x00invalid";
    assert!(decode(garbage).is_err());
}

#[test]
fn test_decode_empty_bytes_returns_error() {
    // bincode requires at minimum a length prefix; empty input is invalid.
    assert!(decode(&[]).is_err());
}

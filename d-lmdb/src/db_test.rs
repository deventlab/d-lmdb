use super::DecodedValue;
use super::decode_live_value;
use super::validate_key;
use super::validate_value;
use crate::Error;
use crate::wire::Value;
use bytes::Bytes;

const MAX_KEY_BYTES: usize = 512;
const MAX_VALUE_BYTES: usize = 1024 * 1024;
// ── validate_key ─────────────────────────────────────────────────────────────

#[test]
fn test_validate_key_accepts_empty_key() {
    // Empty keys are technically valid at the guard level; LMDB accepts them.
    assert!(validate_key(&[], MAX_KEY_BYTES).is_ok());
}

#[test]
fn test_validate_key_accepts_key_at_limit() {
    // Exactly MAX_KEY_BYTES (512) must pass.
    let key = vec![b'k'; MAX_KEY_BYTES];
    assert!(validate_key(&key, MAX_KEY_BYTES).is_ok());
}

#[test]
fn test_validate_key_rejects_key_over_limit() {
    // One byte over the limit must return KeyTooLarge with the actual size.
    let key = vec![b'k'; MAX_KEY_BYTES + 1];
    match validate_key(&key, MAX_KEY_BYTES) {
        Err(Error::KeyTooLarge(n)) => assert_eq!(n, MAX_KEY_BYTES + 1),
        other => panic!("expected KeyTooLarge, got {other:?}"),
    }
}

#[test]
fn test_validate_key_rejects_very_large_key() {
    // Pathologically large key must also be caught.
    let key = vec![b'x'; 10_000];
    assert!(matches!(
        validate_key(&key, MAX_KEY_BYTES),
        Err(Error::KeyTooLarge(_))
    ));
}

// ── validate_value ────────────────────────────────────────────────────────────

#[test]
fn test_validate_value_accepts_empty_value() {
    // Empty values are valid (delete semantics use absence, not empty bytes).
    assert!(validate_value(&[], MAX_VALUE_BYTES).is_ok());
}

#[test]
fn test_validate_value_accepts_value_at_limit() {
    // Exactly MAX_VALUE_BYTES (1 MiB) must pass.
    let value = vec![b'v'; MAX_VALUE_BYTES];
    assert!(validate_value(&value, MAX_VALUE_BYTES).is_ok());
}

#[test]
fn test_validate_value_rejects_value_over_limit() {
    // One byte over 1 MiB must return ValueTooLarge with the actual size.
    let value = vec![b'v'; MAX_VALUE_BYTES + 1];
    match validate_value(&value, MAX_VALUE_BYTES) {
        Err(Error::ValueTooLarge(n)) => assert_eq!(n, MAX_VALUE_BYTES + 1),
        other => panic!("expected ValueTooLarge, got {other:?}"),
    }
}

// ── decode_live_value ────────────────────────────────────────────────────────
//
// Shared by fetch_local/get_linearizable/get_lease. This is the exact bug that
// slipped through before: get_linearizable returned the raw wire envelope
// (tag byte included) instead of the decoded payload. Every case here is
// checked against the *encoded* bytes (via Value::encode_raw/encode_ttl), not
// hand-built payloads, so a regression here fails the same way it would in
// production — an extra leading byte, not just "wrong enum variant".

#[test]
fn test_decode_live_value_non_ttl_strips_envelope() {
    // No leading tag byte in the returned payload — this is precisely what
    // get_linearizable failed to do.
    let encoded = Value::encode_raw(b"world");
    let decoded = decode_live_value(&encoded, 1_000).unwrap();
    assert_eq!(decoded, DecodedValue::Present(Bytes::from_static(b"world")));
}

#[test]
fn test_decode_live_value_ttl_not_yet_expired_returns_payload() {
    let encoded = Value::encode_ttl(b"world", Some(2_000));
    let decoded = decode_live_value(&encoded, 1_000).unwrap();
    assert_eq!(decoded, DecodedValue::Present(Bytes::from_static(b"world")));
}

#[test]
fn test_decode_live_value_ttl_exactly_at_expiry_is_expired() {
    // Boundary: expires_at == now_secs must be treated as expired (`<=`, not `<`).
    let encoded = Value::encode_ttl(b"world", Some(1_000));
    let decoded = decode_live_value(&encoded, 1_000).unwrap();
    assert_eq!(decoded, DecodedValue::Expired);
}

#[test]
fn test_decode_live_value_ttl_past_expiry_is_expired() {
    let encoded = Value::encode_ttl(b"world", Some(500));
    let decoded = decode_live_value(&encoded, 1_000).unwrap();
    assert_eq!(decoded, DecodedValue::Expired);
}

#[test]
fn test_decode_live_value_empty_payload_roundtrips() {
    // Empty values are valid (see test_validate_value_accepts_empty_value) —
    // must not be confused with "no bytes at all" (corrupt).
    let encoded = Value::encode_raw(b"");
    let decoded = decode_live_value(&encoded, 1_000).unwrap();
    assert_eq!(decoded, DecodedValue::Present(Bytes::new()));
}

#[test]
fn test_decode_live_value_rejects_unrecognized_tag_byte() {
    let corrupt = vec![0xFF, b'x'];
    match decode_live_value(&corrupt, 1_000) {
        Err(Error::Storage(msg)) => assert!(msg.contains("invalid tag byte")),
        other => panic!("expected Error::Storage, got {other:?}"),
    }
}

#[test]
fn test_decode_live_value_rejects_empty_bytes() {
    // No tag byte at all — must error, not panic on an out-of-bounds index.
    assert!(decode_live_value(&[], 1_000).is_err());
}

#[test]
fn test_decode_live_value_rejects_truncated_ttl_header() {
    // Tag says TTL but there aren't 8 bytes for expires_at — must error, not panic.
    let truncated = vec![0x01, 0x00, 0x00];
    assert!(decode_live_value(&truncated, 1_000).is_err());
}

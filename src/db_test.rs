use super::validate_key;
use super::validate_value;
use crate::Error;

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

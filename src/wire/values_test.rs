use super::Value;

// ── encode_raw ───────────────────────────────────────────────────────────────

#[test]
fn test_encode_raw_prepends_tag_byte() {
    // First byte must be 0x00 (TAG_RAW); payload follows immediately.
    let encoded = Value::encode_raw(b"hello");
    assert_eq!(encoded[0], 0x00);
    assert_eq!(&encoded[1..], b"hello");
}

#[test]
fn test_encode_raw_empty_payload() {
    // Empty payload encodes to a single tag byte.
    let encoded = Value::encode_raw(b"");
    assert_eq!(encoded, vec![0x00]);
}

// ── encode_ttl ───────────────────────────────────────────────────────────────

#[test]
fn test_encode_ttl_with_expiry_produces_correct_layout() {
    // Layout: [0x01][8-byte big-endian expires_at][payload]
    let ts: u64 = 1_700_000_000;
    let encoded = Value::encode_ttl(b"data", Some(ts));
    assert_eq!(encoded[0], 0x01);
    assert_eq!(&encoded[1..9], &ts.to_be_bytes());
    assert_eq!(&encoded[9..], b"data");
}

#[test]
fn test_encode_ttl_none_falls_back_to_raw_encoding() {
    // No expiry → behaves identically to encode_raw.
    let encoded = Value::encode_ttl(b"data", None);
    assert_eq!(encoded, Value::encode_raw(b"data"));
}

#[test]
fn test_encode_ttl_zero_expiry_is_already_expired() {
    // expires_at = 0 means Jan 1 1970; value is immediately stale.
    // Encoding still succeeds — expiry check is the reader's responsibility.
    let encoded = Value::encode_ttl(b"stale", Some(0));
    assert_eq!(encoded[0], 0x01);
    let expires_at = u64::from_be_bytes(encoded[1..9].try_into().unwrap());
    assert_eq!(expires_at, 0);
}

#[test]
fn test_encode_ttl_max_expiry() {
    // Largest possible timestamp must encode without overflow or truncation.
    let encoded = Value::encode_ttl(b"v", Some(u64::MAX));
    let expires_at = u64::from_be_bytes(encoded[1..9].try_into().unwrap());
    assert_eq!(expires_at, u64::MAX);
}

// ── decode ───────────────────────────────────────────────────────────────────

#[test]
fn test_decode_raw_returns_payload() {
    let encoded = Value::encode_raw(b"hello");
    match Value::decode(&encoded).unwrap() {
        Value::Raw(payload) => assert_eq!(payload, b"hello"),
        Value::Ttl { .. } => panic!("expected Raw"),
    }
}

#[test]
fn test_decode_ttl_returns_expires_at_and_payload() {
    let ts: u64 = 9_999_999_999;
    let encoded = Value::encode_ttl(b"world", Some(ts));
    match Value::decode(&encoded).unwrap() {
        Value::Ttl { expires_at, payload } => {
            assert_eq!(expires_at, ts);
            assert_eq!(payload, b"world");
        }
        Value::Raw(_) => panic!("expected Ttl"),
    }
}

#[test]
fn test_decode_empty_bytes_returns_err() {
    // No tag byte present — cannot determine encoding.
    assert!(Value::decode(&[]).is_err());
}

#[test]
fn test_decode_unknown_tag_returns_err() {
    // 0x02 is not a valid tag; decode must reject it.
    assert!(Value::decode(&[0x02, 0x01, 0x02]).is_err());
}

#[test]
fn test_decode_ttl_truncated_header_returns_err() {
    // TTL header needs 9 bytes (1 tag + 8 timestamp); fewer must fail.
    let truncated = &[0x01, 0x00, 0x00, 0x00]; // only 4 bytes total
    assert!(Value::decode(truncated).is_err());
}

#[test]
fn test_decode_ttl_exact_header_no_payload() {
    // Exactly 9 bytes (tag + timestamp, empty payload) is valid.
    let ts: u64 = 42;
    let mut buf = vec![0x01];
    buf.extend_from_slice(&ts.to_be_bytes());
    match Value::decode(&buf).unwrap() {
        Value::Ttl { expires_at, payload } => {
            assert_eq!(expires_at, 42);
            assert_eq!(payload, b"");
        }
        Value::Raw(_) => panic!("expected Ttl"),
    }
}

// ── round-trip ───────────────────────────────────────────────────────────────

#[test]
fn test_roundtrip_raw() {
    let payload = b"round-trip raw";
    let encoded = Value::encode_raw(payload);
    match Value::decode(&encoded).unwrap() {
        Value::Raw(p) => assert_eq!(p, payload),
        _ => panic!("expected Raw"),
    }
}

#[test]
fn test_roundtrip_ttl() {
    let payload = b"round-trip ttl";
    let ts: u64 = 1_234_567_890;
    let encoded = Value::encode_ttl(payload, Some(ts));
    match Value::decode(&encoded).unwrap() {
        Value::Ttl { expires_at, payload: p } => {
            assert_eq!(expires_at, ts);
            assert_eq!(p, payload);
        }
        _ => panic!("expected Ttl"),
    }
}

#[test]
fn test_roundtrip_ttl_none_decodes_as_raw() {
    // encode_ttl(None) must decode as Raw — callers must not see TTL headers.
    let payload = b"no ttl";
    let encoded = Value::encode_ttl(payload, None);
    match Value::decode(&encoded).unwrap() {
        Value::Raw(p) => assert_eq!(p, payload),
        _ => panic!("expected Raw when expires_at is None"),
    }
}

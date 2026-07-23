//! Value envelope: tag-prefixed binary layout for stored values.
//!
//! Every value written to LMDB begins with a one-byte tag that declares
//! its layout.  Readers must call `decode` before interpreting any value;
//! writers must call `encode_raw` or `encode_ttl` rather than writing
//! raw bytes directly.
//!
//! Wire format for d-lmdb stored keys and values.
//!
//! # Key namespace
//!
//! Internal keys are prefixed with `0x00`, which is never valid as the first
//! byte of a user key.  This lets the state machine fast-path all user traffic
//! with a single `key[0] == 0x00` branch.
//!
//!   [0x00][name...]   — reserved internal key  (e.g. b"\x00batch")
//!   [anything else]   — user key
//!
//! # Value envelope
//!
//! Every stored value begins with a one-byte tag that declares its layout:
//!
//!   [0x00][payload...]                         — raw value (no TTL)
//!   [0x01][expires_at: u64 big-endian][payload...]  — TTL value

#[cfg(test)]
#[path = "values_test.rs"]
mod values_test;

const TAG_RAW: u8 = 0x00;
const TAG_TTL: u8 = 0x01;

const TTL_HEADER_LEN: usize = 1 + 8; // tag + u64 expires_at

pub enum Value<'a> {
    Raw(&'a [u8]),
    Ttl { expires_at: u64, payload: &'a [u8] },
}

impl<'a> Value<'a> {
    /// Wrap a plain value.
    pub(crate) fn encode_raw(payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(1 + payload.len());
        buf.push(TAG_RAW);
        buf.extend_from_slice(payload);
        buf
    }

    /// Wrap a value with absolute expiry (Unix secs).
    pub(crate) fn encode_ttl(
        payload: &[u8],
        expires_at: Option<u64>,
    ) -> Vec<u8> {
        let Some(ts) = expires_at else {
            return Self::encode_raw(payload);
        };
        let mut buf = Vec::with_capacity(TTL_HEADER_LEN + payload.len());
        buf.push(TAG_TTL);
        buf.extend_from_slice(&ts.to_be_bytes());
        buf.extend_from_slice(payload);
        buf
    }

    /// Decode stored bytes back into an Envelope.
    /// Returns Err if the tag byte is unrecognized.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Value<'_>, ()> {
        match bytes.first() {
            Some(&TAG_RAW) => Ok(Value::Raw(&bytes[1..])),
            Some(&TAG_TTL) => {
                if bytes.len() < TTL_HEADER_LEN {
                    return Err(());
                }
                let expires_at = u64::from_be_bytes(bytes[1..9].try_into().unwrap());
                Ok(Value::Ttl {
                    expires_at,
                    payload: &bytes[TTL_HEADER_LEN..],
                })
            }
            _ => Err(()),
        }
    }
}

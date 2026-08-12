//! `WireU256` — a raw EVM `uint256` on the wire.
//!
//! Same byte-string treatment as [`crate::WireFloat`]: 32 big-endian
//! bytes, serde-encoded via `serialize_bytes` / `deserialize_bytes`, so
//! under CBOR this is a single 32-byte byte-string field. The value
//! round-trips bit-for-bit — no decimal parse, no float anywhere. The
//! crate stays pure-Rust; consumers convert with
//! `alloy::primitives::U256::from_be_bytes(wire.0)` (one line) in their
//! own repo.

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct WireU256(pub [u8; 32]);

impl WireU256 {
    pub const ZERO: Self = Self([0u8; 32]);

    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// `true` iff the value is the zero sentinel.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 32]
    }
}

impl From<[u8; 32]> for WireU256 {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<WireU256> for [u8; 32] {
    fn from(w: WireU256) -> Self {
        w.0
    }
}

impl Serialize for WireU256 {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for WireU256 {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        de.deserialize_bytes(WireU256Visitor)
    }
}

struct WireU256Visitor;

impl<'de> Visitor<'de> for WireU256Visitor {
    type Value = WireU256;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a 32-byte big-endian uint256 byte string")
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<WireU256, E> {
        if v.len() != 32 {
            return Err(E::custom(format!("expected 32 bytes, got {}", v.len())));
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(v);
        Ok(WireU256(out))
    }

    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<WireU256, E> {
        self.visit_bytes(&v)
    }

    // Accept a u8 sequence too, mirroring WireFloat — lets a frame
    // round-trip through serde_json for ad-hoc debug.
    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<WireU256, A::Error> {
        let mut out = [0u8; 32];
        for slot in &mut out {
            *slot = seq
                .next_element::<u8>()?
                .ok_or_else(|| de::Error::custom("short byte sequence: need 32 bytes"))?;
        }
        if seq.next_element::<u8>()?.is_some() {
            return Err(de::Error::custom("long byte sequence: need exactly 32"));
        }
        Ok(WireU256(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbor_round_trip_is_32_byte_byte_string() {
        // Full-entropy pattern so a truncation or endianness slip cannot
        // round-trip by accident.
        let mut bytes = [0u8; 32];
        let mut v: u8 = 1;
        for b in &mut bytes {
            *b = v;
            v = v.wrapping_add(37);
        }
        let w = WireU256::from_bytes(bytes);
        let mut buf = Vec::new();
        ciborium::into_writer(&w, &mut buf).unwrap();
        // CBOR byte-string of length 32 = 2-byte head (0x58 0x20) + 32
        // bytes payload. Total 34 bytes.
        assert_eq!(buf.len(), 34, "raw cbor bytes = {buf:02x?}");
        assert_eq!(
            buf[0], 0x58,
            "expected byte-string major type with 1-byte length"
        );
        assert_eq!(buf[1], 0x20, "expected length 32");
        let back: WireU256 = ciborium::from_reader(&buf[..]).unwrap();
        assert_eq!(w, back);
        assert_eq!(back.0, bytes, "payload must be bit-for-bit identical");
    }

    #[test]
    fn cbor_rejects_wrong_length() {
        let mut buf = Vec::new();
        ciborium::into_writer(&serde_bytes_compat::Bytes(vec![1u8; 20]), &mut buf).unwrap();
        let result: Result<WireU256, _> = ciborium::from_reader(&buf[..]);
        assert!(result.is_err(), "20-byte payload should be rejected");
    }

    #[test]
    fn zero_is_the_default_and_reports_is_zero() {
        assert_eq!(WireU256::default(), WireU256::ZERO);
        assert!(WireU256::ZERO.is_zero());
        assert!(!WireU256::from_bytes([1u8; 32]).is_zero());
    }

    mod serde_bytes_compat {
        use serde::{Serialize, Serializer};
        pub struct Bytes(pub Vec<u8>);
        impl Serialize for Bytes {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_bytes(&self.0)
            }
        }
    }
}

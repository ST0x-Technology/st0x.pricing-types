//! `WireFloat` — a Rain Float on the wire.
//!
//! Internally just a 32-byte byte string — the same packed
//! representation Solidity sees as `bytes32`. Serde encodes/decodes
//! via `serialize_bytes` / `deserialize_bytes`, so under CBOR this is
//! a single byte-string field with no parse round-trip and no
//! precision loss.
//!
//! Conversion to `rain_math_float::Float` for arithmetic deliberately
//! lives in the consumer (one-liner — `Float::from_raw(B256::from(w.0))`)
//! so this crate stays pure-Rust and pulls no Foundry / submodules.

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct WireFloat(pub [u8; 32]);

impl WireFloat {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for WireFloat {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<WireFloat> for [u8; 32] {
    fn from(w: WireFloat) -> Self {
        w.0
    }
}

impl Serialize for WireFloat {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for WireFloat {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        de.deserialize_bytes(WireFloatVisitor)
    }
}

struct WireFloatVisitor;

impl<'de> Visitor<'de> for WireFloatVisitor {
    type Value = WireFloat;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a 32-byte Rain Float byte string")
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<WireFloat, E> {
        if v.len() != 32 {
            return Err(E::custom(format!("expected 32 bytes, got {}", v.len())));
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(v);
        Ok(WireFloat(out))
    }

    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<WireFloat, E> {
        self.visit_bytes(&v)
    }

    // Some encoders (notably serde_json) hand bytes through as a
    // sequence of u8 — accept that shape too so a frame can be
    // round-tripped through JSON for ad-hoc debug.
    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<WireFloat, A::Error> {
        let mut out = [0u8; 32];
        for slot in &mut out {
            *slot = seq
                .next_element::<u8>()?
                .ok_or_else(|| de::Error::custom("short byte sequence: need 32 bytes"))?;
        }
        if seq.next_element::<u8>()?.is_some() {
            return Err(de::Error::custom("long byte sequence: need exactly 32"));
        }
        Ok(WireFloat(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbor_round_trip_is_32_byte_byte_string() {
        let w = WireFloat::from_bytes([0xab; 32]);
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
        let back: WireFloat = ciborium::from_reader(&buf[..]).unwrap();
        assert_eq!(w, back);
    }

    #[test]
    fn cbor_rejects_wrong_length() {
        let mut buf = Vec::new();
        ciborium::into_writer(&serde_bytes_compat::Bytes(vec![1u8; 16]), &mut buf).unwrap();
        let result: Result<WireFloat, _> = ciborium::from_reader(&buf[..]);
        assert!(result.is_err(), "16-byte payload should be rejected");
    }

    /// Tiny inline helper to encode `Vec<u8>` as a CBOR byte string for
    /// the test above without adding the `serde_bytes` dep.
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

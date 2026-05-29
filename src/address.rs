//! `WireAddress` — a 20-byte EVM address on the wire.
//!
//! Same byte-string treatment as [`crate::WireFloat`]: serde encodes via
//! `serialize_bytes` / `deserialize_bytes`, so under CBOR this is a
//! single 20-byte byte-string field. The crate stays pure-Rust — no
//! `alloy` dependency — so consumers convert with
//! `alloy::primitives::Address::from(wire.0)` (one line) in their own
//! repo.

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct WireAddress(pub [u8; 20]);

impl WireAddress {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }
}

impl From<[u8; 20]> for WireAddress {
    fn from(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }
}

impl From<WireAddress> for [u8; 20] {
    fn from(w: WireAddress) -> Self {
        w.0
    }
}

impl Serialize for WireAddress {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for WireAddress {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        de.deserialize_bytes(WireAddressVisitor)
    }
}

struct WireAddressVisitor;

impl<'de> Visitor<'de> for WireAddressVisitor {
    type Value = WireAddress;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a 20-byte EVM address byte string")
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<WireAddress, E> {
        if v.len() != 20 {
            return Err(E::custom(format!("expected 20 bytes, got {}", v.len())));
        }
        let mut out = [0u8; 20];
        out.copy_from_slice(v);
        Ok(WireAddress(out))
    }

    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<WireAddress, E> {
        self.visit_bytes(&v)
    }

    // Accept a u8 sequence too, mirroring WireFloat — lets a frame
    // round-trip through serde_json for ad-hoc debug.
    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<WireAddress, A::Error> {
        let mut out = [0u8; 20];
        for slot in &mut out {
            *slot = seq
                .next_element::<u8>()?
                .ok_or_else(|| de::Error::custom("short byte sequence: need 20 bytes"))?;
        }
        if seq.next_element::<u8>()?.is_some() {
            return Err(de::Error::custom("long byte sequence: need exactly 20"));
        }
        Ok(WireAddress(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbor_round_trip_is_20_byte_byte_string() {
        let w = WireAddress::from_bytes([0xab; 20]);
        let mut buf = Vec::new();
        ciborium::into_writer(&w, &mut buf).unwrap();
        // CBOR byte-string of length 20 = 1-byte head (0x54) + 20 payload.
        assert_eq!(buf.len(), 21, "raw cbor bytes = {buf:02x?}");
        assert_eq!(buf[0], 0x54, "expected byte-string major type, length 20");
        let back: WireAddress = ciborium::from_reader(&buf[..]).unwrap();
        assert_eq!(w, back);
    }

    #[test]
    fn cbor_rejects_wrong_length() {
        let mut buf = Vec::new();
        ciborium::into_writer(&serde_bytes_compat::Bytes(vec![1u8; 32]), &mut buf).unwrap();
        let result: Result<WireAddress, _> = ciborium::from_reader(&buf[..]);
        assert!(result.is_err(), "32-byte payload should be rejected");
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

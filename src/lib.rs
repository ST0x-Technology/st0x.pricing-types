//! Wire types for the st0x.pricing service and its consumers.
//!
//! Wire format is **CBOR** (RFC 8949) over WebSocket and over HTTP for
//! the `/snapshot` REST endpoint. All Rain Floats travel as raw 32-byte
//! byte strings (`WireFloat`) — same packed representation Solidity
//! sees as `bytes32`, no parse round-trip, no precision loss.
//!
//! This crate is intentionally pure-Rust: zero dependency on
//! `rain-math-float`, zero submodules, zero Foundry. Consumers that
//! want to do Float arithmetic open the 32 bytes via
//! `rain_math_float::Float::from_raw(B256::from(wire.0))` (one line)
//! and pay the forge build cost in their own repo, not transitively
//! through this one.
//!
//! See `docs/wire-format.md` for framing rules, session lifecycle,
//! close codes, and version policy.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

pub mod address;
pub mod float;
pub mod u256;

pub use address::WireAddress;
pub use float::WireFloat;
pub use u256::WireU256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Venue {
    Bebop,
    Raindex,
    Hook,
}

impl fmt::Display for Venue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Bebop => "bebop",
            Self::Raindex => "raindex",
            Self::Hook => "hook",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    StaleSource,
    UnknownAsset,
    ModelError,
    Internal,
}

/// Symbol = uppercase Alpaca ticker (`COIN`, `TSLA`, ...).
pub type Symbol = String;

/// A single directional-rate quote for an asset, signed off by the model.
///
/// The model emits two independent rates — one per swap direction — both
/// already incorporating whatever spread policy the model chose. Neither
/// rate is "the price": each is the price the model would honour for an
/// input of the named token going to an output of the other.
///
/// * `rate_base_to_quote`: amount of `quote` you receive per 1 unit of
///   `base` input. (Whole-token units; the wire float is unit-free.)
/// * `rate_quote_to_base`: amount of `base` you receive per 1 unit of
///   `quote` input.
///
/// Consumers must NOT invert one to derive the other — that would treat
/// the model's spread as if it were symmetric and discard the per-direction
/// decision. The only legitimate `1/x` happens at protocol-adapter
/// boundaries that require both rates in `quote-per-base` units (e.g.
/// Bebop's level format), and that flip is unit conversion, not pricing.
///
/// `base` is the asset's on-chain token; `quote` is the settlement
/// currency (e.g. USDC on Base). Carrying the canonical pair on the wire
/// means consumers match by address instead of guessing which side is the
/// quote token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub asset: Symbol,
    pub chain_id: u64,
    pub base: WireAddress,
    pub quote: WireAddress,
    pub rate_base_to_quote: WireFloat,
    pub rate_quote_to_base: WireFloat,
    pub expiry_unix_ms: i64,
    pub source_ts_unix_ms: i64,
    /// NAV ratio of the wt vault backing `base`, as the raw `uint256`
    /// returned by the vault's `convertToAssets(1 share)` — the exact
    /// on-chain value the model priced this quote against. Travels as
    /// 32 big-endian bytes so downstream venues can assert bit-for-bit
    /// equality against the vault at settlement. Zero is the sentinel
    /// for "no ratio": `base` is not a vault token (e.g. USDC) and no
    /// settlement assertion applies. A real vault NAV ratio is never
    /// zero.
    #[serde(default)]
    pub nav_ratio: WireU256,
}

/// A coherent point-in-time snapshot of multiple assets for a venue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub snapshot_ts_unix_ms: i64,
    pub venue: Venue,
    pub model_version: String,
    pub prices: Vec<Quote>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    Price(PriceFrame),
    Error(ErrorFrame),
    Ping(PingFrame),
    Halt(HaltFrame),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    Subscribe(SubscribeFrame),
    Unsubscribe(UnsubscribeFrame),
    Pong(PongFrame),
}

/// A live price push for one asset on one venue. The two rates are per-
/// direction for the pair `(base, quote)` on `chain_id` — see [`Quote`]
/// for the semantics and why consumers must not invert one to derive the
/// other.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceFrame {
    pub asset: Symbol,
    pub venue: Venue,
    pub chain_id: u64,
    pub base: WireAddress,
    pub quote: WireAddress,
    pub rate_base_to_quote: WireFloat,
    pub rate_quote_to_base: WireFloat,
    pub expiry_unix_ms: i64,
    pub model_version: String,
    pub source_ts_unix_ms: i64,
    /// NAV ratio of the wt vault backing `base` — see [`Quote::nav_ratio`]
    /// for the exact semantics and the zero sentinel.
    #[serde(default)]
    pub nav_ratio: WireU256,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorFrame {
    pub code: ErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset: Option<Symbol>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ok_unix_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Explicit per-asset quote halt (RAI-702). Pushed when an asset's halt
/// state changes, and once per subscribed asset on subscribe to convey the
/// current state. Distinct from [`ErrorFrame`]/staleness: a halt is an
/// intentional, ops- or NAV-step-triggered pause that consumers MUST honour
/// by not quoting the asset (declining RFQs, skipping level publication)
/// until a frame with `halted = false` arrives. The wrapped vault NAV can
/// step on a dividend deposit; a quote signed just before the step and
/// settled just after is stale, so the producer halts the asset around the
/// step and resumes once repriced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaltFrame {
    pub asset: Symbol,
    pub chain_id: u64,
    pub base: WireAddress,
    pub quote: WireAddress,
    pub halted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingFrame {
    pub ts_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PongFrame {
    pub ts_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeFrame {
    pub consumer: String,
    pub assets: Vec<Symbol>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnsubscribeFrame {
    pub assets: Vec<Symbol>,
}

/// Helper for `Utc::now()` in milliseconds — used by server and clients.
pub fn now_unix_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Convert a `DateTime<Utc>` to ms since epoch.
pub fn to_unix_ms(t: DateTime<Utc>) -> i64 {
    t.timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cbor<T: Serialize>(v: &T) -> Vec<u8> {
        let mut buf = Vec::new();
        ciborium::into_writer(v, &mut buf).unwrap();
        buf
    }

    fn from_cbor<T: for<'de> Deserialize<'de>>(buf: &[u8]) -> T {
        ciborium::from_reader(buf).unwrap()
    }

    /// A NAV ratio with a distinct value in every byte, so a truncated,
    /// reordered, or lossily-converted round-trip cannot pass.
    fn nav_ratio_pattern() -> WireU256 {
        let mut bytes = [0u8; 32];
        let mut v: u8 = 3;
        for b in &mut bytes {
            *b = v;
            v = v.wrapping_add(41);
        }
        WireU256::from_bytes(bytes)
    }

    #[test]
    fn server_frame_round_trip_price() {
        let frame = ServerFrame::Price(PriceFrame {
            asset: "COIN".into(),
            venue: Venue::Bebop,
            chain_id: 8453,
            base: WireAddress::from_bytes([0x11; 20]),
            quote: WireAddress::from_bytes([0x22; 20]),
            rate_base_to_quote: WireFloat::from_bytes([0x42; 32]),
            rate_quote_to_base: WireFloat::from_bytes([0x43; 32]),
            expiry_unix_ms: 1_715_000_030_000,
            model_version: "0.1.0".into(),
            source_ts_unix_ms: 1_714_999_970_000,
            nav_ratio: nav_ratio_pattern(),
        });
        let buf = cbor(&frame);
        let back: ServerFrame = from_cbor(&buf);
        match back {
            ServerFrame::Price(p) => {
                assert_eq!(p.asset, "COIN");
                assert_eq!(p.venue, Venue::Bebop);
                assert_eq!(p.chain_id, 8453);
                assert_eq!(p.base, WireAddress::from_bytes([0x11; 20]));
                assert_eq!(p.quote, WireAddress::from_bytes([0x22; 20]));
                assert_eq!(p.rate_base_to_quote, WireFloat::from_bytes([0x42; 32]));
                assert_eq!(p.rate_quote_to_base, WireFloat::from_bytes([0x43; 32]));
                assert_eq!(p.nav_ratio, nav_ratio_pattern());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn quote_round_trip_preserves_nav_ratio_exactly() {
        let quote = Quote {
            asset: "wtCOIN".into(),
            chain_id: 8453,
            base: WireAddress::from_bytes([0x11; 20]),
            quote: WireAddress::from_bytes([0x22; 20]),
            rate_base_to_quote: WireFloat::from_bytes([0x42; 32]),
            rate_quote_to_base: WireFloat::from_bytes([0x43; 32]),
            expiry_unix_ms: 1_715_000_030_000,
            source_ts_unix_ms: 1_714_999_970_000,
            nav_ratio: nav_ratio_pattern(),
        };
        let back: Quote = from_cbor(&cbor(&quote));
        assert_eq!(back.nav_ratio.0, nav_ratio_pattern().0);
    }

    #[test]
    fn price_frame_without_nav_ratio_decodes_to_zero_sentinel() {
        // A frame from a producer that predates `nav_ratio` has no such
        // map key. Strip the key from a freshly-encoded frame to get that
        // exact wire shape, then decode: the field must default to the
        // zero sentinel ("no ratio / non-vault token").
        let frame = ServerFrame::Price(PriceFrame {
            asset: "COIN".into(),
            venue: Venue::Bebop,
            chain_id: 8453,
            base: WireAddress::from_bytes([0x11; 20]),
            quote: WireAddress::from_bytes([0x22; 20]),
            rate_base_to_quote: WireFloat::from_bytes([0x42; 32]),
            rate_quote_to_base: WireFloat::from_bytes([0x43; 32]),
            expiry_unix_ms: 1_715_000_030_000,
            model_version: "0.1.0".into(),
            source_ts_unix_ms: 1_714_999_970_000,
            nav_ratio: nav_ratio_pattern(),
        });
        let value: ciborium::Value = from_cbor(&cbor(&frame));
        let ciborium::Value::Map(mut entries) = value else {
            panic!("ServerFrame::Price must encode as a CBOR map");
        };
        let before = entries.len();
        entries.retain(|(k, _)| k.as_text() != Some("nav_ratio"));
        assert_eq!(entries.len(), before - 1, "nav_ratio key must be present");
        let back: ServerFrame = from_cbor(&cbor(&ciborium::Value::Map(entries)));
        match back {
            ServerFrame::Price(p) => {
                assert!(p.nav_ratio.is_zero());
                assert_eq!(p.asset, "COIN");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn client_frame_round_trip_subscribe() {
        let frame = ClientFrame::Subscribe(SubscribeFrame {
            consumer: "bebop".into(),
            assets: vec!["COIN".into(), "TSLA".into()],
        });
        let buf = cbor(&frame);
        let back: ClientFrame = from_cbor(&buf);
        match back {
            ClientFrame::Subscribe(s) => {
                assert_eq!(s.consumer, "bebop");
                assert_eq!(s.assets, vec!["COIN", "TSLA"]);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn price_frame_wire_size_bounded() {
        // Sanity check against wire bloat. ~291 bytes with the canonical
        // pair (chain_id + two 20-byte addresses) and the 34-byte
        // `nav_ratio` byte string plus their CBOR map keys; the 360
        // ceiling leaves headroom for a long model_version (e.g. a git
        // sha). A sharp regression past this means a stringly-typed
        // field crept in.
        let frame = ServerFrame::Price(PriceFrame {
            asset: "COIN".into(),
            venue: Venue::Bebop,
            chain_id: 8453,
            base: WireAddress::from_bytes([0x11; 20]),
            quote: WireAddress::from_bytes([0x22; 20]),
            rate_base_to_quote: WireFloat::from_bytes([0x42; 32]),
            rate_quote_to_base: WireFloat::from_bytes([0x43; 32]),
            expiry_unix_ms: 1_715_000_030_000,
            model_version: "0.1.0".into(),
            source_ts_unix_ms: 1_714_999_970_000,
            nav_ratio: nav_ratio_pattern(),
        });
        let buf = cbor(&frame);
        assert!(
            buf.len() < 360,
            "frame ballooned to {} bytes; cbor = {:02x?}",
            buf.len(),
            buf
        );
    }
}

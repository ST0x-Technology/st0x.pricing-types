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
pub mod trading_state;
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

/// The trading session of the asset's listing exchange that a quote was
/// priced in. Unrelated to [`Venue`], the consumer a frame is for.
///
/// US exchanges use `Premarket`, `Rth` and `Afterhours`, and `Closed`
/// outside them. Exchanges without extended hours (the EU exchanges) only
/// use `Rth` for continuous trading and `Closed`. Adding a variant fails decode of the whole frame in
/// consumers built against an older version of this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTag {
    Premarket,
    Rth,
    Afterhours,
    Closed,
}

impl SessionTag {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Premarket => "premarket",
            Self::Rth => "rth",
            Self::Afterhours => "afterhours",
            Self::Closed => "closed",
        }
    }
}

/// The session a quote was priced in, with that session's bounds on the
/// asset's listing exchange, in UTC milliseconds since the Unix epoch.
///
/// For an open session the bounds are the current sub-window: for example
/// `Premarket` ends where `Rth` starts. For `Closed`, `start_unix_ms` is the
/// previous session close and `end_unix_ms` the next session open.
///
/// Every session satisfies `start_unix_ms <= source_ts_unix_ms <=
/// end_unix_ms`, and an open session ends after `source_ts_unix_ms`. A
/// `Closed` bound the producer cannot know is set to `source_ts_unix_ms`, so a
/// bound equal to it means unknown, not a session boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteSession {
    pub tag: SessionTag,
    pub start_unix_ms: i64,
    pub end_unix_ms: i64,
}

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
/// The `underlying_rate_*` pair mirrors these two but prices the vault's
/// underlying ERC4626 asset (the offchain stock) instead of the vault
/// token `base`. It is DEFINED as the served vault rate un-scaled by the
/// NAV ratio, so a consumer deriving the vault price on-chain
/// (`underlying * live convertToAssets(1 share)`) reproduces the SERVED
/// vault rate — including any no-cross clamp the model applied — rather
/// than trusting a signed vault rate. See the field docs below.
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
    /// Exclusive settlement deadline in UTC milliseconds since the Unix epoch.
    /// Producers must supply a positive `Some` for executable session quotes.
    /// Consumers must refuse execution for `None`, nonpositive values, or
    /// `now_ms >= deadline`. Quote freshness remains independently required;
    /// this deadline must not replace or refresh the source timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_deadline_unix_ms: Option<i64>,
    pub source_ts_unix_ms: i64,
    /// NAV ratio of the wt vault backing `base`, as the raw `uint256`
    /// returned by the vault's `convertToAssets(1 share)` — the exact
    /// on-chain value the model priced this quote against. Travels as
    /// 32 big-endian bytes so downstream venues can assert bit-for-bit
    /// equality against the vault at settlement. Zero is the sentinel
    /// for "no ratio": `base` is not a vault token (e.g. USDC) and no
    /// settlement assertion applies. A real vault NAV ratio is never
    /// zero, so a consumer that knows `base` IS a vault token must
    /// treat zero as an upstream fault and refuse it rather than skip
    /// the assertion.
    #[serde(default)]
    pub nav_ratio: WireU256,
    /// Directional rate for the vault's underlying ERC4626 asset (the
    /// offchain stock), `quote` per 1 unit of underlying. Same
    /// directionality and spread policy as `rate_base_to_quote`. DEFINED
    /// as the served (post-clamp) vault rate divided by the NAV *factor*
    /// (`nav_ratio / 10^underlying_decimals`, NOT the raw `nav_ratio`
    /// uint256), so that `underlying * convertToAssets(oneShare) / oneShare`
    /// — equivalently the Rain Float `underlying * erc4626-convert-to-assets(
    /// vault, 1)`, which normalizes internally — reproduces the SERVED
    /// `rate_base_to_quote`, the exact rate the model published, clamps
    /// included, and a consumer can derive the vault price atomically instead
    /// of trusting the signed vault rate. See `docs/wire-format.md` for the
    /// canonical formula. Crucially
    /// this is the CLAMP-CONSISTENT underlying, not necessarily the raw
    /// stock mark: when the model's no-cross clamp binds, this rate
    /// reflects the clamped vault rate, so deriving from it cannot
    /// reconstruct an unclamped (self-crossing) quote. When `base` is not
    /// a vault token (`nav_ratio` zero) there is no separate underlying and
    /// this equals `rate_base_to_quote`. (Decimal-float division is not a
    /// bit-exact inverse of multiplication, so the derived vault rate
    /// matches the served one to Float precision, not necessarily
    /// bit-for-bit.) Frames from producers that predate the field decode
    /// to the zero Float via `#[serde(default)]`; that all-zero sentinel
    /// means "not carried", distinct from a real (always non-zero) stock
    /// rate.
    #[serde(default)]
    pub underlying_rate_base_to_quote: WireFloat,
    /// Directional rate for the vault's underlying ERC4626 asset, units of
    /// underlying per 1 unit of `quote` — the reverse-direction counterpart
    /// of [`Self::underlying_rate_base_to_quote`], mirroring
    /// `rate_quote_to_base`. See that field for the vault-derivation and
    /// zero-sentinel semantics.
    #[serde(default)]
    pub underlying_rate_quote_to_base: WireFloat,
    /// Session the quote was priced in, on the asset's listing exchange. Producers
    /// always send it. `None` means the frame came from a producer that
    /// predates the field; a consumer that signs or gates on the session must
    /// refuse such a frame rather than derive a session elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<QuoteSession>,
}

/// A coherent point-in-time snapshot of multiple assets for a venue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub snapshot_ts_unix_ms: i64,
    pub venue: Venue,
    pub model_version: String,
    pub prices: Vec<Quote>,
}

// Boxing `PriceFrame` would change every consumer's pattern match for a
// value that is built once per frame and moved straight into the encoder.
#[allow(clippy::large_enum_variant)]
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
    /// Exclusive UTC settlement deadline; see [`Quote::execution_deadline_unix_ms`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_deadline_unix_ms: Option<i64>,
    pub model_version: String,
    pub source_ts_unix_ms: i64,
    /// NAV ratio of the wt vault backing `base` — see [`Quote::nav_ratio`]
    /// for the exact semantics and the zero sentinel.
    #[serde(default)]
    pub nav_ratio: WireU256,
    /// Underlying-asset directional rate — see
    /// [`Quote::underlying_rate_base_to_quote`] for the vault-derivation
    /// and zero-sentinel semantics.
    #[serde(default)]
    pub underlying_rate_base_to_quote: WireFloat,
    /// Reverse-direction underlying rate — see
    /// [`Quote::underlying_rate_quote_to_base`].
    #[serde(default)]
    pub underlying_rate_quote_to_base: WireFloat,
    /// Session the quote was priced in; see [`Quote::session`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<QuoteSession>,
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

    fn rth_session() -> QuoteSession {
        QuoteSession {
            tag: SessionTag::Rth,
            start_unix_ms: 1_714_973_400_000,
            end_unix_ms: 1_715_003_000_000,
        }
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
            execution_deadline_unix_ms: Some(1_715_003_000_000),
            model_version: "0.1.0".into(),
            source_ts_unix_ms: 1_714_999_970_000,
            nav_ratio: nav_ratio_pattern(),
            underlying_rate_base_to_quote: WireFloat::from_bytes([0x44; 32]),
            underlying_rate_quote_to_base: WireFloat::from_bytes([0x45; 32]),
            session: Some(rth_session()),
        });
        let buf = cbor(&frame);
        let ciborium::Value::Map(mut legacy_entries) = from_cbor(&buf) else {
            panic!("price frame must encode as map");
        };
        legacy_entries.retain(|(key, _)| key.as_text() != Some("execution_deadline_unix_ms"));
        let legacy: ServerFrame = from_cbor(&cbor(&ciborium::Value::Map(legacy_entries)));
        assert!(matches!(
            legacy,
            ServerFrame::Price(PriceFrame {
                execution_deadline_unix_ms: None,
                ..
            })
        ));
        let back: ServerFrame = from_cbor(&buf);
        match back {
            ServerFrame::Price(p) => {
                assert_eq!(p.asset, "COIN");
                assert_eq!(p.session, Some(rth_session()));
                assert_eq!(p.execution_deadline_unix_ms, Some(1_715_003_000_000));
                assert_eq!(p.venue, Venue::Bebop);
                assert_eq!(p.chain_id, 8453);
                assert_eq!(p.base, WireAddress::from_bytes([0x11; 20]));
                assert_eq!(p.quote, WireAddress::from_bytes([0x22; 20]));
                assert_eq!(p.rate_base_to_quote, WireFloat::from_bytes([0x42; 32]));
                assert_eq!(p.rate_quote_to_base, WireFloat::from_bytes([0x43; 32]));
                assert_eq!(p.nav_ratio, nav_ratio_pattern());
                assert_eq!(
                    p.underlying_rate_base_to_quote,
                    WireFloat::from_bytes([0x44; 32])
                );
                assert_eq!(
                    p.underlying_rate_quote_to_base,
                    WireFloat::from_bytes([0x45; 32])
                );
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
            execution_deadline_unix_ms: Some(1_715_003_000_000),
            source_ts_unix_ms: 1_714_999_970_000,
            nav_ratio: nav_ratio_pattern(),
            underlying_rate_base_to_quote: WireFloat::from_bytes([0x44; 32]),
            underlying_rate_quote_to_base: WireFloat::from_bytes([0x45; 32]),
            session: Some(rth_session()),
        };
        let back: Quote = from_cbor(&cbor(&quote));
        assert_eq!(back.nav_ratio.0, nav_ratio_pattern().0);
        assert_eq!(back.session, Some(rth_session()));
        assert_eq!(
            back.execution_deadline_unix_ms,
            quote.execution_deadline_unix_ms
        );
        let ciborium::Value::Map(mut entries) = from_cbor(&cbor(&quote)) else {
            panic!("quote must encode as map");
        };
        entries.retain(|(key, _)| key.as_text() != Some("execution_deadline_unix_ms"));
        let legacy: Quote = from_cbor(&cbor(&ciborium::Value::Map(entries)));
        assert_eq!(legacy.execution_deadline_unix_ms, None);
    }

    #[test]
    fn quote_round_trip_preserves_underlying_rates_exactly() {
        // Distinct patterns per field so a swapped, truncated, or dropped
        // underlying rate cannot pass. The underlying rates are the stock
        // price the model priced against and downstream may derive the
        // vault price from, so any lossy round-trip is a correctness bug.
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
            underlying_rate_base_to_quote: WireFloat::from_bytes([0x44; 32]),
            underlying_rate_quote_to_base: WireFloat::from_bytes([0x45; 32]),
            session: None,
            execution_deadline_unix_ms: None,
        };
        let back: Quote = from_cbor(&cbor(&quote));
        assert_eq!(
            back.underlying_rate_base_to_quote,
            WireFloat::from_bytes([0x44; 32])
        );
        assert_eq!(
            back.underlying_rate_quote_to_base,
            WireFloat::from_bytes([0x45; 32])
        );
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
            execution_deadline_unix_ms: None,
            model_version: "0.1.0".into(),
            source_ts_unix_ms: 1_714_999_970_000,
            nav_ratio: nav_ratio_pattern(),
            underlying_rate_base_to_quote: WireFloat::from_bytes([0x44; 32]),
            underlying_rate_quote_to_base: WireFloat::from_bytes([0x45; 32]),
            session: None,
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
    fn price_frame_without_underlying_rates_decodes_to_zero_default() {
        // A frame from a producer that predates the underlying rates has no
        // such map keys. Strip both from a freshly-encoded frame to get that
        // exact wire shape, then decode: each must default to the all-zero
        // WireFloat ("not carried"), leaving the rest of the frame intact.
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
            underlying_rate_base_to_quote: WireFloat::from_bytes([0x44; 32]),
            underlying_rate_quote_to_base: WireFloat::from_bytes([0x45; 32]),
            session: None,
            execution_deadline_unix_ms: None,
        });
        let value: ciborium::Value = from_cbor(&cbor(&frame));
        let ciborium::Value::Map(mut entries) = value else {
            panic!("ServerFrame::Price must encode as a CBOR map");
        };
        let before = entries.len();
        entries.retain(|(k, _)| {
            !matches!(
                k.as_text(),
                Some("underlying_rate_base_to_quote" | "underlying_rate_quote_to_base")
            )
        });
        assert_eq!(
            entries.len(),
            before - 2,
            "both underlying rate keys must be present"
        );
        let back: ServerFrame = from_cbor(&cbor(&ciborium::Value::Map(entries)));
        match back {
            ServerFrame::Price(p) => {
                assert_eq!(p.underlying_rate_base_to_quote, WireFloat::default());
                assert_eq!(p.underlying_rate_quote_to_base, WireFloat::default());
                // The rest of the frame is untouched by the missing keys.
                assert_eq!(p.rate_base_to_quote, WireFloat::from_bytes([0x42; 32]));
                assert_eq!(p.nav_ratio, nav_ratio_pattern());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn quote_without_underlying_rates_decodes_to_zero_default() {
        // REST consumers (/snapshot) decode old producers through `Quote`, not
        // `PriceFrame`, so its two `#[serde(default)]` attributes need their own
        // regression pin. Strip both underlying keys from a freshly-encoded
        // Quote and confirm each defaults to the all-zero WireFloat.
        let quote = Quote {
            asset: "COIN".into(),
            chain_id: 8453,
            base: WireAddress::from_bytes([0x11; 20]),
            quote: WireAddress::from_bytes([0x22; 20]),
            rate_base_to_quote: WireFloat::from_bytes([0x42; 32]),
            rate_quote_to_base: WireFloat::from_bytes([0x43; 32]),
            expiry_unix_ms: 1_715_000_030_000,
            source_ts_unix_ms: 1_714_999_970_000,
            nav_ratio: nav_ratio_pattern(),
            underlying_rate_base_to_quote: WireFloat::from_bytes([0x44; 32]),
            underlying_rate_quote_to_base: WireFloat::from_bytes([0x45; 32]),
            session: None,
            execution_deadline_unix_ms: None,
        };
        let ciborium::Value::Map(mut entries) = from_cbor::<ciborium::Value>(&cbor(&quote)) else {
            panic!("Quote must encode as a CBOR map");
        };
        let before = entries.len();
        entries.retain(|(k, _)| {
            !matches!(
                k.as_text(),
                Some("underlying_rate_base_to_quote" | "underlying_rate_quote_to_base")
            )
        });
        assert_eq!(
            entries.len(),
            before - 2,
            "both underlying rate keys must be present"
        );
        let back: Quote = from_cbor(&cbor(&ciborium::Value::Map(entries)));
        assert_eq!(back.underlying_rate_base_to_quote, WireFloat::default());
        assert_eq!(back.underlying_rate_quote_to_base, WireFloat::default());
        // The rest of the quote is untouched by the missing keys.
        assert_eq!(back.rate_base_to_quote, WireFloat::from_bytes([0x42; 32]));
        assert_eq!(back.nav_ratio, nav_ratio_pattern());
    }

    fn price_frame(session: Option<QuoteSession>) -> PriceFrame {
        PriceFrame {
            asset: "COIN".into(),
            venue: Venue::Raindex,
            chain_id: 8453,
            base: WireAddress::from_bytes([0x11; 20]),
            quote: WireAddress::from_bytes([0x22; 20]),
            rate_base_to_quote: WireFloat::from_bytes([0x42; 32]),
            rate_quote_to_base: WireFloat::from_bytes([0x43; 32]),
            expiry_unix_ms: 1_715_000_030_000,
            execution_deadline_unix_ms: Some(1_715_003_000_000),
            model_version: "0.1.0".into(),
            source_ts_unix_ms: 1_714_999_970_000,
            nav_ratio: nav_ratio_pattern(),
            underlying_rate_base_to_quote: WireFloat::from_bytes([0x44; 32]),
            underlying_rate_quote_to_base: WireFloat::from_bytes([0x45; 32]),
            session,
        }
    }

    #[test]
    fn session_tags_use_their_wire_names() {
        for tag in [
            SessionTag::Premarket,
            SessionTag::Rth,
            SessionTag::Afterhours,
            SessionTag::Closed,
        ] {
            assert_eq!(
                serde_json::to_value(tag).unwrap(),
                serde_json::Value::String(tag.as_str().into())
            );
            let back: SessionTag = from_cbor(&cbor(&tag));
            assert_eq!(back, tag);
        }
    }

    #[test]
    fn price_frame_session_round_trips_in_json() {
        let frame = ServerFrame::Price(price_frame(Some(QuoteSession {
            tag: SessionTag::Closed,
            start_unix_ms: 1_714_996_800_000,
            end_unix_ms: 1_715_068_800_000,
        })));
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(
            json["session"],
            serde_json::json!({
                "tag": "closed",
                "start_unix_ms": 1_714_996_800_000_i64,
                "end_unix_ms": 1_715_068_800_000_i64,
            })
        );
        let ServerFrame::Price(back) = serde_json::from_value(json).unwrap() else {
            panic!("wrong variant");
        };
        assert_eq!(back.session.map(|s| s.tag), Some(SessionTag::Closed));
    }

    #[test]
    fn price_frame_without_session_decodes_to_none_and_omits_the_key() {
        let buf = cbor(&ServerFrame::Price(price_frame(None)));
        let ciborium::Value::Map(entries) = from_cbor(&buf) else {
            panic!("ServerFrame::Price must encode as a CBOR map");
        };
        assert!(entries.iter().all(|(k, _)| k.as_text() != Some("session")));
        let ServerFrame::Price(back) = from_cbor(&buf) else {
            panic!("wrong variant");
        };
        assert_eq!(back.session, None);
    }

    /// The frame and quote shapes of v0.8.0, the last release without a
    /// session. Consumers still on it must keep decoding frames that carry one.
    mod v0_8 {
        use crate::{Symbol, Venue, WireAddress, WireFloat, WireU256};
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        pub enum ServerFrame {
            Price(PriceFrame),
        }

        #[derive(Deserialize)]
        pub struct PriceFrame {
            pub asset: Symbol,
            pub venue: Venue,
            pub chain_id: u64,
            pub base: WireAddress,
            pub quote: WireAddress,
            pub rate_base_to_quote: WireFloat,
            pub rate_quote_to_base: WireFloat,
            pub expiry_unix_ms: i64,
            #[serde(default)]
            pub execution_deadline_unix_ms: Option<i64>,
            pub model_version: String,
            pub source_ts_unix_ms: i64,
            #[serde(default)]
            pub nav_ratio: WireU256,
            #[serde(default)]
            pub underlying_rate_base_to_quote: WireFloat,
            #[serde(default)]
            pub underlying_rate_quote_to_base: WireFloat,
        }

        #[derive(Deserialize)]
        pub struct SnapshotQuote {
            pub asset: Symbol,
            pub chain_id: u64,
            pub base: WireAddress,
            pub quote: WireAddress,
            pub rate_base_to_quote: WireFloat,
            pub rate_quote_to_base: WireFloat,
            pub expiry_unix_ms: i64,
            #[serde(default)]
            pub execution_deadline_unix_ms: Option<i64>,
            pub source_ts_unix_ms: i64,
            #[serde(default)]
            pub nav_ratio: WireU256,
            #[serde(default)]
            pub underlying_rate_base_to_quote: WireFloat,
            #[serde(default)]
            pub underlying_rate_quote_to_base: WireFloat,
        }
    }

    #[test]
    fn v0_8_consumers_decode_frames_that_carry_a_session() {
        let frame = ServerFrame::Price(price_frame(Some(rth_session())));
        let v0_8::ServerFrame::Price(old) = from_cbor(&cbor(&frame));
        assert_eq!(old.asset, "COIN");
        assert_eq!(old.venue, Venue::Raindex);
        assert_eq!(old.chain_id, 8453);
        assert_eq!(old.base, WireAddress::from_bytes([0x11; 20]));
        assert_eq!(old.quote, WireAddress::from_bytes([0x22; 20]));
        assert_eq!(old.rate_base_to_quote, WireFloat::from_bytes([0x42; 32]));
        assert_eq!(old.rate_quote_to_base, WireFloat::from_bytes([0x43; 32]));
        assert_eq!(old.expiry_unix_ms, 1_715_000_030_000);
        assert_eq!(old.execution_deadline_unix_ms, Some(1_715_003_000_000));
        assert_eq!(old.model_version, "0.1.0");
        assert_eq!(old.source_ts_unix_ms, 1_714_999_970_000);
        assert_eq!(old.nav_ratio, nav_ratio_pattern());
        assert_eq!(
            old.underlying_rate_base_to_quote,
            WireFloat::from_bytes([0x44; 32])
        );
        assert_eq!(
            old.underlying_rate_quote_to_base,
            WireFloat::from_bytes([0x45; 32])
        );

        let v0_8::ServerFrame::Price(old) =
            serde_json::from_value(serde_json::to_value(&frame).unwrap()).unwrap();
        assert_eq!(old.source_ts_unix_ms, 1_714_999_970_000);
    }

    #[test]
    fn v0_8_consumers_decode_snapshot_quotes_that_carry_a_session() {
        let frame = price_frame(Some(rth_session()));
        let snapshot = Snapshot {
            snapshot_ts_unix_ms: 1_715_000_000_000,
            venue: Venue::Raindex,
            model_version: "0.1.0".into(),
            prices: vec![Quote {
                asset: frame.asset,
                chain_id: frame.chain_id,
                base: frame.base,
                quote: frame.quote,
                rate_base_to_quote: frame.rate_base_to_quote,
                rate_quote_to_base: frame.rate_quote_to_base,
                expiry_unix_ms: frame.expiry_unix_ms,
                execution_deadline_unix_ms: frame.execution_deadline_unix_ms,
                source_ts_unix_ms: frame.source_ts_unix_ms,
                nav_ratio: frame.nav_ratio,
                underlying_rate_base_to_quote: frame.underlying_rate_base_to_quote,
                underlying_rate_quote_to_base: frame.underlying_rate_quote_to_base,
                session: frame.session,
            }],
        };
        let buf = cbor(&snapshot);
        let ciborium::Value::Map(entries) = from_cbor(&buf) else {
            panic!("Snapshot must encode as a CBOR map");
        };
        let prices = entries
            .into_iter()
            .find(|(k, _)| k.as_text() == Some("prices"))
            .map(|(_, v)| v)
            .expect("prices key");
        let old: Vec<v0_8::SnapshotQuote> = from_cbor(&cbor(&prices));
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].asset, "COIN");
        assert_eq!(old[0].chain_id, 8453);
        assert_eq!(old[0].base, WireAddress::from_bytes([0x11; 20]));
        assert_eq!(old[0].quote, WireAddress::from_bytes([0x22; 20]));
        assert_eq!(old[0].rate_base_to_quote, WireFloat::from_bytes([0x42; 32]));
        assert_eq!(old[0].rate_quote_to_base, WireFloat::from_bytes([0x43; 32]));
        assert_eq!(old[0].expiry_unix_ms, 1_715_000_030_000);
        assert_eq!(old[0].execution_deadline_unix_ms, Some(1_715_003_000_000));
        assert_eq!(old[0].source_ts_unix_ms, 1_714_999_970_000);
        assert_eq!(old[0].nav_ratio, nav_ratio_pattern());
        assert_eq!(
            old[0].underlying_rate_base_to_quote,
            WireFloat::from_bytes([0x44; 32])
        );
        assert_eq!(
            old[0].underlying_rate_quote_to_base,
            WireFloat::from_bytes([0x45; 32])
        );
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
        // The canonical CBOR frame is 585 bytes: two 20-byte addresses, four
        // 32-byte rates, a 32-byte NAV ratio, integer timestamps,
        // a populated execution deadline, a 40-character model SHA, a session
        // with two integer bounds, and map keys.
        // The 610-byte ceiling leaves 25 bytes of headroom. A breach can mean
        // a binary address or float became stringly typed; check the encoding
        // before increasing the budget.
        let frame = ServerFrame::Price(PriceFrame {
            asset: "COIN".into(),
            venue: Venue::Bebop,
            chain_id: 8453,
            base: WireAddress::from_bytes([0x11; 20]),
            quote: WireAddress::from_bytes([0x22; 20]),
            rate_base_to_quote: WireFloat::from_bytes([0x42; 32]),
            rate_quote_to_base: WireFloat::from_bytes([0x43; 32]),
            expiry_unix_ms: 1_715_000_030_000,
            execution_deadline_unix_ms: Some(1_715_003_000_000),
            model_version: "0123456789abcdef0123456789abcdef01234567".into(),
            source_ts_unix_ms: 1_714_999_970_000,
            nav_ratio: nav_ratio_pattern(),
            underlying_rate_base_to_quote: WireFloat::from_bytes([0x44; 32]),
            underlying_rate_quote_to_base: WireFloat::from_bytes([0x45; 32]),
            session: Some(rth_session()),
        });
        let buf = cbor(&frame);
        assert!(
            buf.len() < 610,
            "frame ballooned to {} bytes; cbor = {:02x?}",
            buf.len(),
            buf
        );
    }
}

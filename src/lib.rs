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
    /// zero, so a consumer that knows `base` IS a vault token must
    /// treat zero as an upstream fault and refuse it rather than skip
    /// the assertion.
    #[serde(default)]
    pub nav_ratio: WireU256,
    /// Trading session this quote was priced in, as the tag the oracle
    /// signs into the on-chain context: `"rth"`, `"premarket"` or
    /// `"afterhours"`.
    ///
    /// The producer owns market-hours truth — it already clamps
    /// [`Self::expiry_unix_ms`] to the session boundary — so stating the
    /// session on the wire removes the second, independently-refreshed
    /// calendar consumers otherwise keep. Two calendars means two
    /// refresh loops and a disagreement no test in either repo can
    /// catch; one calendar means the quote and the session it was
    /// priced in can never come from different views of "now".
    ///
    /// A closed market produces no quote at all, so the tag domain is
    /// exactly those three values by construction — there is no
    /// `"closed"` tag because there is no quote to carry it.
    ///
    /// `None` means the producer predates v0.7.0 and says nothing about
    /// the session; a consumer that needs one falls back to its own
    /// calendar. A consumer that does not recognise the tag must fail
    /// closed and refuse the quote rather than guess: an unrecognised
    /// session is one an on-chain strategy will reject anyway.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Start of [`Self::session`] in UTC milliseconds since the epoch —
    /// the bound the oracle signs alongside the tag.
    ///
    /// The three session fields are **all-or-nothing**: a producer emits
    /// the tag, the start and the end together, or none of them. They are
    /// three separate `Option`s because the wire format has no way to
    /// express one optional triple, not because a producer may pick and
    /// choose. A partial statement — a tag without both bounds, or bounds
    /// without a tag — is a fault, and a consumer must refuse the quote
    /// rather than fill the gap from its own calendar: substituting the
    /// consumer's answer for a statement the producer declined to make is
    /// exactly the two-calendar divergence these fields exist to remove.
    /// `st0x-oracle-server` enforces this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_start_unix_ms: Option<i64>,
    /// End of [`Self::session`] in UTC milliseconds since the epoch: the
    /// same boundary [`Self::expiry_unix_ms`] is clamped to, from the
    /// same source, so the two can never disagree.
    ///
    /// A consumer must treat a `session_end_unix_ms` already in the past
    /// as a fault and refuse the quote — it means the producer's view of
    /// the session outlived the session itself, and anything signed off
    /// it would be rejected on-chain regardless.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_end_unix_ms: Option<i64>,
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
    /// Trading session this frame was priced in — see [`Quote::session`]
    /// for the tag domain, why the producer owns it, and what `None`
    /// means.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Start of `session` in UTC milliseconds — see
    /// [`Quote::session_start_unix_ms`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_start_unix_ms: Option<i64>,
    /// End of `session` in UTC milliseconds — see
    /// [`Quote::session_end_unix_ms`], including why a consumer must
    /// refuse a bound already in the past.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_end_unix_ms: Option<i64>,
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
            session: None,
            session_start_unix_ms: None,
            session_end_unix_ms: None,
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
            session: None,
            session_start_unix_ms: None,
            session_end_unix_ms: None,
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
            session: None,
            session_start_unix_ms: None,
            session_end_unix_ms: None,
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
        // Sanity check against wire bloat, measured on the largest
        // shape a frame can take: every optional field populated.
        // ~399 bytes with the canonical pair (chain_id + two 20-byte
        // addresses), the 34-byte `nav_ratio` byte string and the three
        // session fields, plus their CBOR map keys; the 470 ceiling
        // leaves headroom for a long model_version (e.g. a git sha). A
        // sharp regression past this means a stringly-typed field crept
        // in.
        let frame = ServerFrame::Price(v07_price_frame_with_session());
        let buf = cbor(&frame);
        assert!(
            buf.len() < 470,
            "frame ballooned to {} bytes; cbor = {:02x?}",
            buf.len(),
            buf
        );
    }

    /// v0.6.0's `PriceFrame` verbatim, mirrored locally so both
    /// compatibility directions can be exercised without pulling the
    /// previous tag in as a second dependency. Field set, field order
    /// and serde attributes are a byte-for-byte copy of the v0.6.0
    /// definition — if this drifts, the compatibility tests below stop
    /// meaning anything.
    #[derive(Debug, Serialize, Deserialize)]
    struct V06PriceFrame {
        asset: Symbol,
        venue: Venue,
        chain_id: u64,
        base: WireAddress,
        quote: WireAddress,
        rate_base_to_quote: WireFloat,
        rate_quote_to_base: WireFloat,
        expiry_unix_ms: i64,
        model_version: String,
        source_ts_unix_ms: i64,
        #[serde(default)]
        nav_ratio: WireU256,
    }

    /// v0.6.0's `Quote` verbatim — see [`V06PriceFrame`].
    #[derive(Debug, Serialize, Deserialize)]
    struct V06Quote {
        asset: Symbol,
        chain_id: u64,
        base: WireAddress,
        quote: WireAddress,
        rate_base_to_quote: WireFloat,
        rate_quote_to_base: WireFloat,
        expiry_unix_ms: i64,
        source_ts_unix_ms: i64,
        #[serde(default)]
        nav_ratio: WireU256,
    }

    /// v0.6.0's `ServerFrame` restricted to the variant under test; the
    /// internal tagging is what makes the unknown-key tolerance
    /// non-obvious, so it must be exercised through the enum and not
    /// through the bare struct.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum V06ServerFrame {
        Price(V06PriceFrame),
    }

    const SESSION_START_MS: i64 = 1_714_996_800_000;
    const SESSION_END_MS: i64 = 1_715_020_200_000;

    fn v07_price_frame_with_session() -> PriceFrame {
        PriceFrame {
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
            // Deliberately NOT "rth": "rth" is the value a bug that
            // hardcoded the common case would also produce.
            session: Some("premarket".into()),
            // Both bounds differ from each other and from every other
            // i64 on the frame, so a crossed key or a copied field is
            // a failure and not a coincidence.
            session_start_unix_ms: Some(SESSION_START_MS),
            session_end_unix_ms: Some(SESSION_END_MS),
        }
    }

    fn v07_price_frame_without_session() -> PriceFrame {
        PriceFrame {
            session: None,
            session_start_unix_ms: None,
            session_end_unix_ms: None,
            ..v07_price_frame_with_session()
        }
    }

    fn v06_price_frame() -> V06PriceFrame {
        V06PriceFrame {
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
        }
    }

    fn map_keys(buf: &[u8]) -> Vec<String> {
        let ciborium::Value::Map(entries) = from_cbor::<ciborium::Value>(buf) else {
            panic!("frame must encode as a CBOR map");
        };
        entries
            .iter()
            .filter_map(|(k, _)| k.as_text().map(str::to_owned))
            .collect()
    }

    #[test]
    fn price_frame_round_trip_preserves_session_fields() {
        let back: ServerFrame =
            from_cbor(&cbor(&ServerFrame::Price(v07_price_frame_with_session())));
        let ServerFrame::Price(p) = back else {
            panic!("wrong variant");
        };
        assert_eq!(p.session.as_deref(), Some("premarket"));
        assert_eq!(p.session_start_unix_ms, Some(SESSION_START_MS));
        assert_eq!(p.session_end_unix_ms, Some(SESSION_END_MS));
        // The pre-existing fields must be untouched by the addition.
        assert_eq!(p.expiry_unix_ms, 1_715_000_030_000);
        assert_eq!(p.source_ts_unix_ms, 1_714_999_970_000);
        assert_eq!(p.nav_ratio, nav_ratio_pattern());
    }

    #[test]
    fn quote_round_trip_preserves_session_fields() {
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
            session: Some("afterhours".into()),
            session_start_unix_ms: Some(SESSION_START_MS),
            session_end_unix_ms: Some(SESSION_END_MS),
        };
        let back: Quote = from_cbor(&cbor(&quote));
        assert_eq!(back.session.as_deref(), Some("afterhours"));
        assert_eq!(back.session_start_unix_ms, Some(SESSION_START_MS));
        assert_eq!(back.session_end_unix_ms, Some(SESSION_END_MS));
        assert_eq!(back.expiry_unix_ms, 1_715_000_030_000);
        assert_eq!(back.nav_ratio, nav_ratio_pattern());
    }

    #[test]
    fn a_partial_session_statement_survives_the_wire_for_the_consumer_to_refuse() {
        // The three fields are all-or-nothing *as a contract*, and no
        // conforming producer emits a partial statement. This pins the
        // encoding underneath that contract: the fields are three
        // separate `Option`s, so a partial statement round-trips
        // faithfully instead of being silently completed or dropped.
        //
        // That matters because the consumer is what enforces the
        // contract — `st0x-oracle-server`'s `wire_session` matches
        // `(Some, Some, Some)` and refuses anything else. It can only
        // refuse a partial statement it can actually observe, so the
        // wire must carry one through unchanged rather than paper over
        // it here.
        let frame = PriceFrame {
            session_start_unix_ms: None,
            ..v07_price_frame_with_session()
        };
        let keys = map_keys(&cbor(&ServerFrame::Price(frame.clone())));
        assert!(keys.contains(&"session".to_owned()));
        assert!(!keys.contains(&"session_start_unix_ms".to_owned()));
        assert!(keys.contains(&"session_end_unix_ms".to_owned()));

        let back: ServerFrame = from_cbor(&cbor(&ServerFrame::Price(frame)));
        let ServerFrame::Price(p) = back else {
            panic!("wrong variant");
        };
        assert_eq!(p.session.as_deref(), Some("premarket"));
        assert_eq!(p.session_start_unix_ms, None);
        assert_eq!(p.session_end_unix_ms, Some(SESSION_END_MS));
    }

    #[test]
    fn v07_frame_without_session_is_byte_identical_to_v06() {
        // The strongest statement of backwards compatibility available:
        // a v0.7.0 producer that has nothing to say about the session
        // does not merely decode on a v0.6.0 consumer, it emits the
        // exact same bytes v0.6.0 would have. `skip_serializing_if`
        // rather than a serialised `null` is what buys this.
        let v07 = cbor(&ServerFrame::Price(v07_price_frame_without_session()));
        let v06 = cbor(&V06ServerFrame::Price(v06_price_frame()));
        assert_eq!(v07, v06, "all-None v0.7.0 frame must be v0.6.0 on the wire");
    }

    #[test]
    fn v07_frame_with_session_decodes_into_v06_types() {
        let buf = cbor(&ServerFrame::Price(v07_price_frame_with_session()));

        // Guard against the test passing for the wrong reason: if the
        // fields never reached the wire, decoding them away below would
        // prove nothing at all.
        let keys = map_keys(&buf);
        for key in ["session", "session_start_unix_ms", "session_end_unix_ms"] {
            assert!(
                keys.contains(&key.to_owned()),
                "{key} missing from the wire"
            );
        }

        let V06ServerFrame::Price(old) = from_cbor::<V06ServerFrame>(&buf);
        assert_eq!(old.asset, "COIN");
        assert_eq!(old.venue, Venue::Bebop);
        assert_eq!(old.chain_id, 8453);
        assert_eq!(old.base, WireAddress::from_bytes([0x11; 20]));
        assert_eq!(old.quote, WireAddress::from_bytes([0x22; 20]));
        assert_eq!(old.rate_base_to_quote, WireFloat::from_bytes([0x42; 32]));
        assert_eq!(old.rate_quote_to_base, WireFloat::from_bytes([0x43; 32]));
        assert_eq!(old.expiry_unix_ms, 1_715_000_030_000);
        assert_eq!(old.model_version, "0.1.0");
        assert_eq!(old.source_ts_unix_ms, 1_714_999_970_000);
        assert_eq!(old.nav_ratio, nav_ratio_pattern());
    }

    #[test]
    fn v06_frame_decodes_into_v07_types_with_absent_session() {
        let buf = cbor(&V06ServerFrame::Price(v06_price_frame()));

        // Same guard in the other direction: the v0.6.0 encoding must
        // genuinely lack the keys, otherwise "decodes to None" is not a
        // statement about missing keys.
        let keys = map_keys(&buf);
        for key in ["session", "session_start_unix_ms", "session_end_unix_ms"] {
            assert!(!keys.contains(&key.to_owned()), "{key} leaked into v0.6.0");
        }

        let back: ServerFrame = from_cbor(&buf);
        let ServerFrame::Price(p) = back else {
            panic!("wrong variant");
        };
        assert_eq!(p.session, None);
        assert_eq!(p.session_start_unix_ms, None);
        assert_eq!(p.session_end_unix_ms, None);
        assert_eq!(p.asset, "COIN");
        assert_eq!(p.expiry_unix_ms, 1_715_000_030_000);
        assert_eq!(p.source_ts_unix_ms, 1_714_999_970_000);
        assert_eq!(p.nav_ratio, nav_ratio_pattern());
    }

    #[test]
    fn v06_quote_decodes_into_v07_quote_with_absent_session() {
        let buf = cbor(&V06Quote {
            asset: "wtCOIN".into(),
            chain_id: 8453,
            base: WireAddress::from_bytes([0x11; 20]),
            quote: WireAddress::from_bytes([0x22; 20]),
            rate_base_to_quote: WireFloat::from_bytes([0x42; 32]),
            rate_quote_to_base: WireFloat::from_bytes([0x43; 32]),
            expiry_unix_ms: 1_715_000_030_000,
            source_ts_unix_ms: 1_714_999_970_000,
            nav_ratio: nav_ratio_pattern(),
        });
        let keys = map_keys(&buf);
        assert!(!keys.contains(&"session".to_owned()));

        let back: Quote = from_cbor(&buf);
        assert_eq!(back.session, None);
        assert_eq!(back.session_start_unix_ms, None);
        assert_eq!(back.session_end_unix_ms, None);
        assert_eq!(back.asset, "wtCOIN");
        assert_eq!(back.nav_ratio, nav_ratio_pattern());
    }
}

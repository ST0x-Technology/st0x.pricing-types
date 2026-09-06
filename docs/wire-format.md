# Wire format

The pricing service speaks **CBOR** (RFC 8949) — over WebSocket for the live
feed and over HTTP for the `/snapshot` and `/resolve` REST endpoints. Every body
has `Content-Type: application/cbor`.

The authoritative schema is the Rust type definitions in
[`src/lib.rs`](../src/lib.rs). Consumers depend on this crate (`Cargo.toml`:
`st0x-pricing-types = { git = "https://github.com/ST0x-Technology/st0x.pricing-types", tag = "v0.2.0" }`)
or mirror its types verbatim — the struct definitions _are_ the contract. This
document covers everything that isn't captured by the struct definitions:
framing, transport rules, error semantics, and version policy.

## Rain Floats on the wire

A Rain `Float` travels as its raw 32-byte packed representation — the same
`bytes32` value Solidity sees on-chain. In CBOR this is a single byte string of
length 32 (major type 2). Encoded payload is 34 bytes:

```
0x58 0x20 <32 bytes>
```

Consumers that need to do Float arithmetic (or convert to a printable decimal)
decode the bytes into `rain_math_float::Float` once at the last hop:

```rust
use alloy::primitives::B256;
use rain_math_float::Float;
use st0x_pricing_types::WireFloat;

fn to_float(w: WireFloat) -> Float {
    Float::from_raw(B256::from(w.0))
}
```

This crate intentionally doesn't bundle the conversion — `rain-math-float` needs
Foundry to compile (its `sol!` macros load `DecimalFloat.json` at compile time),
and consumers that just forward the bytes downstream shouldn't pay that cost. No
f64 ever appears on the wire.

## Raw uint256 values on the wire

A raw EVM `uint256` (`WireU256`) travels as 32 big-endian bytes — the same CBOR
byte-string shape as a Rain Float, so the value round-trips bit-for-bit with no
decimal parse and no float anywhere. Consumers convert with
`alloy::primitives::U256::from_be_bytes(wire.0)`.

The one such field today is `nav_ratio` on `PriceFrame` / `Quote`: the exact
`convertToAssets(1 share)` return of the wt vault backing `base` at the time the
model priced the frame. Downstream venues assert exact equality against the
vault on-chain at settlement, so any lossy representation would break the
assertion. The all-zero value is the sentinel for "no ratio" (non-vault `base`,
e.g. USDC) and means no settlement assertion applies; frames from producers that
predate the field decode to the same sentinel via `#[serde(default)]`.

The sentinel is trusted blind only by consumers that cannot know whether `base`
is a vault token. A consumer that CAN know (a maker quoting a vault-sourced
asset, an order deployed on a wt pair, a hook with a vault configured for the
pool) must refuse a zero ratio outright rather than skip the assertion: a real
vault NAV ratio is never zero, so zero-for-a-known-vault means something
upstream is broken and forwarding it would let the fill settle unprotected.

## Trading session on the wire

`PriceFrame` / `Quote` carry three optional session fields: `session` (a text
tag), `session_start_unix_ms` and `session_end_unix_ms` (UTC milliseconds since
the epoch). Together they say which trading session the producer priced the
quote in and where that session's bounds are.

The producer already owns market-hours truth — it clamps `expiry_unix_ms` to the
session boundary — so `session_end_unix_ms` is that same boundary from that same
source. Consumers that need a session (the oracle signs one into the on-chain
context) previously re-derived it from their own independently-refreshed
calendar. Two calendars means two refresh loops and a disagreement no test in
either repo can catch; stating the session on the wire collapses that to one.

The tag domain is `rth`, `premarket` and `afterhours` — the exact set the
on-chain strategies accept. There is no `closed` tag: a closed market produces
no quote at all, so the domain is closed by construction rather than by a lossy
mapping.

Consumer rules:

- **Absent (`None`)** means the producer predates v0.7.0 and is saying nothing.
  Fall back to your own calendar, and count the fallback — once the counter is
  flat at zero the local calendar can be deleted.
- **An unrecognised tag** must fail closed: refuse the quote rather than guess.
  A tag this crate doesn't list is one an on-chain strategy would reject anyway.
- **A `session_end_unix_ms` already in the past** is a fault, not a stale-but-
  usable bound. Refuse the quote: the producer's view of the session outlived
  the session, and anything signed off it reverts on-chain.

All three are independently optional. A producer that knows the session and its
end but not its start says exactly that; the fields are three `Option`s, not one
all-or-nothing group.

Wire compatibility runs both ways and is covered by tests. The fields use
`skip_serializing_if = "Option::is_none"`, so a v0.7.0 producer with nothing to
say emits bytes **byte-identical** to v0.6.0; and a v0.6.0 frame, which simply
lacks the keys, decodes into the v0.7.0 types with all three `None`. Older
consumers ignore the unknown map keys as usual.

## WebSocket framing

URL: `wss://<host>/ws`. The upgrade request must carry
`Authorization: Bearer <api-key>`. Bad / missing auth is rejected pre-upgrade
with `401`.

All WebSocket data frames are **binary**. Text frames are rejected. Both
directions speak CBOR encodings of the structs in this crate:

| Direction       | Type          | Payload                                   |
| --------------- | ------------- | ----------------------------------------- |
| Client → Server | `ClientFrame` | one of `subscribe`, `unsubscribe`, `pong` |
| Server → Client | `ServerFrame` | one of `price`, `error`, `ping`           |

The variant is encoded as a `type` field inside the CBOR map.

### Session lifecycle

1. **Upgrade** with the `Authorization` header.
2. **First frame must be `subscribe`** — the server closes with
   `4000 expected_subscribe` if it gets anything else, and times out the session
   if no frame arrives in 10 s.
3. **Subscribe → initial quotes**: for every asset listed in `subscribe` that
   has a fresh cached mark, the server emits one `price` frame immediately.
   Assets that aren't yet warmed up emit no frame; the consumer should retry /
   wait.
4. **Steady state**: server pushes a `price` frame on every fresh poll for every
   asset the session has subscribed to (broadcast bus, filtered per-session).
5. **Heartbeat**: server emits `ping` every 15 s. Client must reply with `pong`
   (same `ts_unix_ms`) within 10 s — otherwise the server closes with
   `4001 heartbeat_timeout`.
6. **Subscribe deltas**: client can send further `subscribe` / `unsubscribe`
   frames at any time after the first; assets must be in the registry.
7. **Close codes**: `4000` bad protocol, `4001` heartbeat timeout, `4002`
   subscribe timeout, `4003` consumer mismatch, `4004` unknown asset, `4999`
   server shutting down.

## REST framing

All REST handlers respond with `application/cbor` for both success and error
bodies — consumers run a single CBOR decoder regardless of HTTP status.

| Endpoint                           | Auth   | Success body                                                 | Error body                 |
| ---------------------------------- | ------ | ------------------------------------------------------------ | -------------------------- |
| `GET /health`                      | none   | `200 ok` / `503 …` (plain text — health-check tool friendly) |                            |
| `GET /snapshot?assets=A,B&venue=v` | Bearer | `Snapshot`                                                   | `ErrorBody`                |
| `GET /resolve?chain_id=N&token=…`  | none   | `{symbol}`                                                   | `{error, chain_id, token}` |
| `GET /metrics`                     | none   | Prometheus text exposition                                   |                            |

`/snapshot` is the **single coherent point-in-time view** for the listed assets
— the cache is read-locked for the duration of the read, so all
`Quote.source_ts_unix_ms` values come from the same snapshot.

Status codes follow the obvious mapping: `429` for `warming` (no fresh quote
yet), `503` for `stale_source`, `404` for unknown asset, `403` for consumer
mismatch, `401` for missing auth.

## Version policy

This crate's `Cargo.toml` version is the wire-contract version:

- `0.x.y`: additive _or_ breaking changes allowed, but every consumer updates in
  lockstep. The price of being internal.
- `1.0.0` onwards: additive only on the wire. Removing a field or changing a tag
  becomes a major bump.

The pricing service binary versions independently (`vX.Y.Z` on `st0x.pricing`).

## Why CBOR

- Binary, ~5× smaller than the equivalent JSON.
- Rain Float is a native byte string — no decimal-string parse / format
  round-trip, no f64 risk anywhere.
- Self-describing: `ciborium::value::Value` decodes any frame for ops inspection
  without a schema file.
- Fully serde-compatible — this crate is a normal
  `#[derive(Serialize, Deserialize)]` set of structs.
- Standardised (IETF RFC 8949); broad cross-language support if a non-Rust
  consumer ever appears.

## Debug tooling

`ciborium::value::Value::deserialized` plus `serde_json::to_string` round-trips
any frame into JSON for hex-dump / curl debugging without needing the schema. A
small `pricing-debug` CLI is the right place to codify this once we feel the
friction.

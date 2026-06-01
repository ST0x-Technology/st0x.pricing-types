# st0x-pricing-types

Wire types for the
[`st0x.pricing`](https://github.com/ST0x-Technology/st0x.pricing) service and
its consumers ([`st0x.bebop`](https://github.com/ST0x-Technology/st0x.bebop),
[`st0x.hook`](https://github.com/ST0x-Technology/st0x.hook),
[`st0x-oracle-server`](https://github.com/ST0x-Technology/st0x-oracle-server)).

Pure Rust, no Foundry, no submodules. The crate is the schema; the struct
definitions are the contract.

## Use

```toml
[dependencies]
st0x-pricing-types = { git = "https://github.com/ST0x-Technology/st0x.pricing-types", tag = "v0.2.0" }
```

```rust
use st0x_pricing_types::{ClientFrame, ServerFrame, WireFloat};

let frame: ServerFrame = ciborium::from_reader(&bytes[..])?;
```

## Converting to `rain_math_float::Float`

This crate intentionally does **not** depend on `rain-math-float` — that crate's
`sol!` macros need Foundry's `forge build` at compile time, and a bytes-only
consumer shouldn't have to pay that. Consumers that need Float arithmetic do the
conversion themselves:

```rust
use alloy::primitives::B256;
use rain_math_float::Float;
use st0x_pricing_types::WireFloat;

let f = Float::from_raw(B256::from(wire.0));
let display = f.format()?;
```

## Versioning

This crate's `Cargo.toml` version is the wire-contract version. See
[`docs/wire-format.md`](docs/wire-format.md) for the full version policy +
framing rules + close codes.

## License

CAL-1.0 with combined-work exception.

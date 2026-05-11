{
  description = "st0x-pricing-types — wire types for st0x.pricing and its consumers";

  inputs = {
    rainix.url = "github:rainlanguage/rainix";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, flake-utils, rainix }:
    flake-utils.lib.eachDefaultSystem (system: {
      # Pure-Rust crate — no Foundry / submodules / sol-shell needed.
      devShells.default = rainix.devShells.${system}.rust-shell;
    });
}

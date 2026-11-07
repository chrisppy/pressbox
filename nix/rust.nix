# SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
# SPDX-License-Identifier: GPL-3.0-only

# The Rust toolchain (fenix, latest stable) and a crane library built on it,
# shared by the packages, checks and dev shell. The toolchain moves when the
# fenix input is updated; Cargo.toml's rust-version is the minimum supported.
{ inputs, ... }:
{
  perSystem =
    { pkgs, system, ... }:
    let
      rustToolchain = inputs.fenix.packages.${system}.stable.withComponents [
        "cargo"
        "clippy"
        "rust-analyzer"
        "rust-src"
        "rustc"
        "rustfmt"
      ];
    in
    {
      _module.args = {
        inherit rustToolchain;
        craneLib = (inputs.crane.mkLib pkgs).overrideToolchain (_: rustToolchain);
      };
    };
}

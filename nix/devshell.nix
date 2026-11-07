# SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
# SPDX-License-Identifier: GPL-3.0-only

_: {
  perSystem =
    {
      pkgs,
      config,
      craneLib,
      ...
    }:
    {
      # The same fenix toolchain the packages build with (cargo, rustc, clippy,
      # rustfmt, rust-analyzer, rust-src), plus the packages' build inputs.
      devShells.default = craneLib.devShell {
        inputsFrom = [ config.packages.pressboxd ];
        packages = [
          pkgs.cargo-deny
          pkgs.just
          pkgs.sqlite
          config.treefmt.build.wrapper
        ];
      };

      formatter = config.treefmt.build.wrapper;
    };
}

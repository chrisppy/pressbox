# SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
# SPDX-License-Identifier: GPL-3.0-only

_: {
  perSystem =
    { pkgs, config, ... }:
    {
      devShells.default = pkgs.mkShell {
        inputsFrom = [ config.packages.pressboxd ];
        packages = [
          pkgs.cargo
          pkgs.clippy
          pkgs.rust-analyzer
          pkgs.rustc
          pkgs.rustfmt
          pkgs.sqlite
          config.treefmt.build.wrapper
        ];
      };

      formatter = config.treefmt.build.wrapper;
    };
}

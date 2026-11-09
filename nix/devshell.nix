# SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
# SPDX-License-Identifier: GPL-3.0-only

_: {
  perSystem =
    {
      pkgs,
      lib,
      config,
      craneLib,
      guiRuntimeLibs,
      ...
    }:
    {
      # The same fenix toolchain the packages build with (cargo, rustc, clippy,
      # rustfmt, rust-analyzer, rust-src), plus the packages' build inputs and
      # the libraries pressbox-iced opens at run time, so `just gui` finds a
      # display. cargo-tarpaulin, for `just coverage`, is Linux-only: its
      # ptrace engine needs no extra toolchain component.
      devShells.default = craneLib.devShell {
        inputsFrom = [ config.packages.pressboxd ];
        packages = [
          pkgs.cargo-deny
          pkgs.just
          pkgs.sqlite
          config.treefmt.build.wrapper
        ]
        ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.cargo-tarpaulin ];
        LD_LIBRARY_PATH = lib.makeLibraryPath guiRuntimeLibs;
      };

      formatter = config.treefmt.build.wrapper;
    };
}

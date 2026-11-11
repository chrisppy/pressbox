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
      gstLibs,
      gstPluginPath,
      ...
    }:
    {
      # The same fenix toolchain the packages build with (cargo, rustc, clippy,
      # rustfmt, rust-analyzer, rust-src), plus the packages' build inputs and
      # the libraries pressbox-iced opens at run time, so `just gui` finds a
      # display, and cargo-tarpaulin for `just coverage`. GStreamer (and its
      # plugins, for `just gui`) for the desktop client's podcast player, and
      # mpv for the terminal client's.
      devShells.default = craneLib.devShell {
        inputsFrom = [ config.packages.pressboxd ];
        packages = [
          pkgs.cargo-deny
          pkgs.just
          pkgs.sqlite
          pkgs.cargo-tarpaulin
          pkgs.pkg-config
          pkgs.mpv
          config.treefmt.build.wrapper
        ]
        ++ gstLibs;
        LD_LIBRARY_PATH = lib.makeLibraryPath guiRuntimeLibs;
        GST_PLUGIN_SYSTEM_PATH_1_0 = gstPluginPath;
      };

      formatter = config.treefmt.build.wrapper;
    };
}

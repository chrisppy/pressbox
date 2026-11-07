# SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
# SPDX-License-Identifier: GPL-3.0-only

# pressboxd and pressbox, built with crane: the workspace's dependencies are
# built once (cargoArtifacts) and shared by both packages and the Clippy check.
_: {
  perSystem =
    {
      pkgs,
      lib,
      craneLib,
      ...
    }:
    let
      cargoToml = lib.importTOML ../Cargo.toml;

      src = lib.fileset.toSource {
        root = ../.;
        fileset = lib.fileset.unions [
          ../Cargo.toml
          ../Cargo.lock
          (craneLib.fileset.commonCargoSources ../crates)
          (craneLib.fileset.commonCargoSources ../clients/tui)
        ];
      };

      commonArgs = {
        inherit src;
        inherit (cargoToml.workspace.package) version;
        pname = "pressbox-workspace";
        strictDeps = true;

        # reqwest (rustls) loads the system CA certificates when a client is
        # built, and the build sandbox has none, so tests that build a client
        # fail with "No CA certificates were loaded from the system".
        # cacert's setup hook points SSL_CERT_FILE at its bundle.
        nativeCheckInputs = [ pkgs.cacert ];
      };

      cargoArtifacts = craneLib.buildDepsOnly commonArgs;

      mkCrate =
        {
          pname,
          description,
          license,
          mainProgram ? pname,
        }:
        craneLib.buildPackage (
          commonArgs
          // {
            inherit pname cargoArtifacts;
            # Applies to both the build and the tests.
            cargoExtraArgs = "--locked --package ${pname}";

            meta = {
              inherit description license mainProgram;
              homepage = "https://github.com/chrisppy/pressbox";
              platforms = lib.platforms.unix;
            };
          }
        );

      pressboxd = mkCrate {
        pname = "pressboxd";
        description = "pressbox RSS/Atom aggregation server";
        license = lib.licenses.agpl3Only;
      };

      pressbox = mkCrate {
        pname = "pressbox";
        description = "Terminal client for pressbox";
        license = lib.licenses.gpl3Only;
      };
    in
    {
      packages = {
        inherit pressboxd pressbox;
        default = pressboxd;
      };

      # Same as `just lint`.
      checks.clippy = craneLib.cargoClippy (
        commonArgs
        // {
          inherit cargoArtifacts;
          cargoClippyExtraArgs = "--workspace --all-targets -- --deny warnings";
        }
      );
    };
}

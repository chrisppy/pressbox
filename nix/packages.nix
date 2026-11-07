# SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
# SPDX-License-Identifier: GPL-3.0-only

_: {
  perSystem =
    { pkgs, lib, ... }:
    let
      cargoToml = lib.importTOML ../Cargo.toml;

      rustSrc = lib.fileset.toSource {
        root = ../.;
        fileset = lib.fileset.unions [
          ../Cargo.toml
          ../Cargo.lock
          ../crates
          ../clients/tui
        ];
      };

      mkCrate =
        {
          pname,
          description,
          license,
          mainProgram ? pname,
        }:
        pkgs.rustPlatform.buildRustPackage {
          inherit pname;
          inherit (cargoToml.workspace.package) version;
          src = rustSrc;
          cargoLock.lockFile = ../Cargo.lock;
          cargoBuildFlags = [
            "--package"
            pname
          ];
          cargoTestFlags = [
            "--package"
            pname
          ];

          # reqwest (rustls) loads the system CA certificates when a client is
          # built, and the build sandbox has none, so tests that build a client
          # fail with "No CA certificates were loaded from the system".
          # cacert's setup hook points SSL_CERT_FILE at its bundle.
          nativeCheckInputs = [ pkgs.cacert ];

          meta = {
            inherit description license mainProgram;
            homepage = "https://github.com/chrisppy/pressbox";
            platforms = lib.platforms.unix;
          };
        };

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
    };
}

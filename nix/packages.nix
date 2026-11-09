# SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
# SPDX-License-Identifier: GPL-3.0-only

# pressboxd, pressbox and pressbox-iced, built with crane. There are two
# dependency builds: cargoArtifacts, shared by pressboxd, the TUI and their
# Clippy check, and guiArtifacts for the desktop client. So pressboxd and the
# TUI never build iced and its graphics stack. (buildDepsOnly can't start
# from other artifacts, so crates both use, such as tokio, build in each.)
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
          (craneLib.fileset.commonCargoSources ../clients/iced)
          ../assets/logo.svg
          ../assets/pressbox-iced.desktop
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

      # Everything but the desktop client.
      serverArgs = commonArgs // {
        cargoExtraArgs = "--locked --workspace --exclude pressbox-iced";
      };
      guiArgs = commonArgs // {
        cargoExtraArgs = "--locked --package pressbox-iced";
      };

      cargoArtifacts = craneLib.buildDepsOnly serverArgs;
      guiArtifacts = craneLib.buildDepsOnly guiArgs;

      # winit and wgpu open these with dlopen at run time instead of linking
      # them, so they go on pressbox-iced's RUNPATH (and in the dev shell).
      guiRuntimeLibs = lib.optionals pkgs.stdenv.hostPlatform.isLinux [
        pkgs.libGL
        pkgs.libxkbcommon
        pkgs.vulkan-loader
        pkgs.wayland
        pkgs.libx11
        pkgs.libxcursor
        pkgs.libxi
        pkgs.libxrandr
      ];

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

      pressbox-iced = craneLib.buildPackage (
        guiArgs
        // {
          pname = "pressbox-iced";
          cargoArtifacts = guiArtifacts;

          postInstall = ''
            install -Dm644 assets/pressbox-iced.desktop -t $out/share/applications
            install -Dm644 assets/logo.svg $out/share/icons/hicolor/scalable/apps/pressbox-iced.svg
          '';

          postFixup = lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
            patchelf --add-rpath ${lib.makeLibraryPath guiRuntimeLibs} $out/bin/pressbox-iced
          '';

          meta = {
            description = "Desktop client for pressbox, built with iced";
            homepage = "https://github.com/chrisppy/pressbox";
            license = lib.licenses.gpl3Only;
            mainProgram = "pressbox-iced";
            platforms = lib.platforms.linux ++ lib.platforms.darwin;
          };
        }
      );
    in
    {
      _module.args = { inherit guiRuntimeLibs; };

      packages = {
        inherit pressboxd pressbox pressbox-iced;
        default = pressboxd;
      };

      # Together the same as `just lint`.
      checks.clippy = craneLib.cargoClippy (
        serverArgs
        // {
          inherit cargoArtifacts;
          cargoClippyExtraArgs = "--all-targets -- --deny warnings";
        }
      );
      checks.clippy-iced = craneLib.cargoClippy (
        guiArgs
        // {
          cargoArtifacts = guiArtifacts;
          cargoClippyExtraArgs = "--all-targets -- --deny warnings";
        }
      );
    };
}

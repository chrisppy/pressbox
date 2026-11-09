# SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
# SPDX-License-Identifier: GPL-3.0-only

# Exports the NixOS and Home Manager modules with this flake's packages as
# defaults. The modules themselves live in ../modules because devfmt loads
# every file in this directory as a flake-parts module.
{ self, lib, ... }:
let
  withPackages =
    module: defaults:
    { pkgs, ... }:
    let
      packages = self.packages.${pkgs.stdenv.hostPlatform.system};
    in
    {
      imports = [ module ];
      config = lib.mapAttrsRecursive (_: name: lib.mkDefault packages.${name}) defaults;
    };

  nixos = withPackages ../modules/nixos.nix {
    services.pressbox.package = "pressboxd";
  };

  homeManager = withPackages ../modules/home-manager.nix {
    programs.pressbox = {
      tui.package = "pressbox";
      desktop.package = "pressbox-iced";
    };
  };
in
{
  flake = {
    nixosModules = {
      pressbox = nixos;
      default = nixos;
    };
    homeManagerModules = {
      pressbox = homeManager;
      default = homeManager;
    };
  };
}

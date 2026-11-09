# SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
# SPDX-License-Identifier: GPL-3.0-only

{
  config,
  lib,
  ...
}:
let
  inherit (lib) mkOption types;
  cfg = config.programs.pressbox;
in
{
  options.programs.pressbox = {
    enable = lib.mkEnableOption "pressbox feed reader clients";

    server = mkOption {
      type = types.str;
      default = "http://127.0.0.1:8470";
      example = "https://rss.example.org";
      description = "URL of the pressboxd server.";
    };

    tokenFile = mkOption {
      type = types.nullOr (types.pathWith { inStore = false; });
      default = null;
      example = "/run/secrets/pressbox-token";
      description = ''
        File holding this user's API token. Leave unset to pair instead:
        `pressbox pair` saves the token to
        `$XDG_STATE_HOME/pressbox/token`, which pressbox-iced and Birdland's
        bar read too.
        Only the path is ever stored in the Nix store.
      '';
    };

    tui = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = "Install the terminal client, `pressbox`.";
      };
      package = mkOption {
        type = types.package;
        description = "The pressbox package.";
      };
    };

    desktop = {
      enable = mkOption {
        type = types.bool;
        default = false;
        description = "Install the desktop client, `pressbox-iced`, with its desktop entry.";
      };
      package = mkOption {
        type = types.package;
        description = "The pressbox-iced package.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    # Read by pressbox, and by Birdland's bar for its feeds indicator.
    xdg.configFile."pressbox/client.json".text = builtins.toJSON (
      {
        inherit (cfg) server;
      }
      // lib.optionalAttrs (cfg.tokenFile != null) {
        inherit (cfg) tokenFile;
      }
    );

    home.packages =
      lib.optional cfg.tui.enable cfg.tui.package
      ++ lib.optional cfg.desktop.enable cfg.desktop.package;
  };
}

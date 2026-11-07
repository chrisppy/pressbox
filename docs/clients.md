<!--
SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
SPDX-License-Identifier: GPL-3.0-only
-->

## Clients (Home Manager)

Import `inputs.pressbox.homeManagerModules.default`:

```nix
{
  programs.pressbox = {
    enable = true;
    server = "https://pressbox.your-tailnet.ts.net";
  };
}
```

This writes `~/.config/pressbox/client.json` and installs `pressbox`. Pairing
saves the token to `$XDG_STATE_HOME/pressbox/token`; set `tokenFile` only to use
a token from somewhere else.

### pressbox (terminal client)

`pressbox [--server URL] [--token-file PATH] [--config PATH]`. Flags and the
`PRESSBOX_SERVER` / `PRESSBOX_TOKEN_FILE` environment variables override
`client.json`. `pressbox pair [--name NAME]` pairs this machine;
`pressbox approve [CODE] [--user U]` approves (or lists) other devices'
requests, and `pressbox whoami` shows the signed-in user and device.

| Key | Action |
| --------------------- | -------------------------------------------- |
| `j` `k` / arrows | Move (scrolls in the reader) |
| `Enter` / `l` / `Tab` | Open feed → item list → reader |
| `Esc` / `h` | Back (clears a search first) |
| `/` | Search titles and text, including read items |
| `m` | Toggle read |
| `s` | Toggle star |
| `o` | Open in browser (marks read) |
| `A` | Mark the selected feed (or all feeds) read |
| `u` | Toggle unread-only / all |
| `r` / `R` | Reload from server / ask server to refetch |
| `q` | Quit |

### In the Birdland bar

[Birdland](https://github.com/chrisppy/birdland)'s bar, bullpen, has a `feeds`
module showing the unread count. Click opens `pressbox` in your terminal, right
click asks the server to fetch every feed, and new items send a notification. It
reads the same `client.json` and token as `pressbox`, so pairing the TUI is all
it needs:

```nix
programs.birdland.bullpen.layout.right = [ "feeds" "clock" ]; # among your modules
```

Its options (`programs.birdland.bullpen.feeds.command`, `interval` and
`notifications`) are in
[Birdland's configuration options](https://github.com/chrisppy/birdland/blob/master/docs/usage.md#configuration-options).

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
a token from somewhere else. Both clients read these files, so pairing one pairs
both.

Set `desktop.enable = true` to also install `pressbox-iced`, the desktop client,
and `tui.enable = false` to leave out the terminal one.

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
| `M` | Mark the selected item and all below it read |
| `u` | Cycle unread / all / read items |
| `r` / `R` | Reload from server / ask server to refetch |
| `p` | Play the episode (or pause / resume it) |
| `V` | Play the episode's video |
| `x` | Stop playing (saves where you were) |
| `,` / `.` | Back 10s / forward 30s |
| `[` / `]` | Previous / next chapter |
| `-` / `+` | Slower / faster |
| `t` | Show the episode's transcript (in the reader) |
| `q` | Quit |

Podcast episodes (items with audio or video) are marked `♪` in the list, `◐`
once started and `▶` while playing. They play in [mpv](https://mpv.io), which
must be installed (or set `player` in `client.json`, or `programs.pressbox.player`
in Home Manager): audio plays in the background, video in mpv's window. The
line above the status shows the time, chapter and speed. The position is saved
on the server every 15 seconds, on pause and on stop, so playing resumes where
you left off on any client; an episode played to the end (or stopped in its
last 30 seconds) is marked read. Opening an episode doesn't mark it read. The
reader lists its chapters, the current one highlighted while it plays, and `t`
swaps the text for the transcript.

### pressbox-iced (desktop client)

A window with three panes: sources (Unread, All items, Starred, tags and feeds,
with unread counts), the item list with search, and the article. Opening an item
marks it read. Its desktop entry is named "pressbox", and its Wayland app ID is
`pressbox-iced`, for window rules.

On a machine that isn't paired yet it asks for the server and a device name and
shows a code to approve, as `pressbox pair` does; it then saves the token where
the TUI looks, and writes `client.json` only if that has no server (so a file
Home Manager manages is left alone). The sidebar can approve another device's
code, ask the server to refetch every feed, and sign out, which deletes the
token.

| Key | Action |
| --------- | ----------------- |
| `j` / `↓` | Next item |
| `k` / `↑` | Previous item |
| `s` | Toggle star |
| `m` | Toggle read |
| `o` | Open in browser |
| `Esc` | Dismiss a message |

Colors are [Birdland](https://github.com/chrisppy/base24-themes), and Birdland
Light when the system is in light mode.

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

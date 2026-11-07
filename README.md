<!--
SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
SPDX-License-Identifier: GPL-3.0-only
-->

<p align="center"><img src="assets/logo.svg" width="128" alt=""></p>

# pressbox

A self-hosted RSS/Atom/JSON Feed aggregator. You declare feeds (and the people
using them) in your NixOS configuration, a small Rust server polls them, and a
terminal client on NixOS and an
[iOS app](https://github.com/chrisppy/pressbox-ios) share read and starred state
through its API. Each client is paired with a short code; there is no website
and no shared secret. [Birdland](https://github.com/chrisppy/birdland)'s bar can
show the unread count.

```
NixOS config ──▶ pressbox.toml ──▶ pressboxd ──▶ SQLite (items, search index, devices)
(services.pressbox.feeds)            │ HTTP API, one token per device
                   ┌───────────────┼────────────────┐
               pressbox      Birdland bar        iOS app
                           pressboxctl (control socket, on the server)
```

| Part | Path | What it is |
| ------------------- | -------------------------------------------------------- | ------------------------------------- |
| `pressboxd` | `crates/pressboxd` | Server: polling, storage, search, API |
| `pressboxctl` | `crates/pressboxd/src/bin` | Pairing, devices, backups, OPML |
| `pressbox-api` | `crates/pressbox-api` | JSON types shared by the Rust crates |
| `pressbox` | `clients/tui` | Terminal client (ratatui) |
| iOS app | [pressbox-ios](https://github.com/chrisppy/pressbox-ios) | SwiftUI client, in its own repo |
| NixOS module | `modules/nixos.nix` | `services.pressbox` |
| Home Manager module | `modules/home-manager.nix` | `programs.pressbox` |

## Server (NixOS)

Add the flake to your system configuration:

```nix
{
  inputs.pressbox = {
    url = "github:chrisppy/pressbox";
    inputs.nixpkgs.follows = "nixpkgs";
  };
}
```

Then import `inputs.pressbox.nixosModules.default` and declare your feeds:

```nix
{
  services.pressbox = {
    enable = true;
    refreshInterval = 1800; # seconds, per feed

    feeds = {
      lwn = "https://lwn.net/headlines/rss"; # shorthand for { url = ...; }
      nixos = {
        url = "https://nixos.org/blog/announcements-rss.xml";
        title = "NixOS";
        tags = [ "nix" ];
        refreshInterval = 3600; # this feed only
      };
    };
  };
}
```

### Users

For a household or friends, declare users. Everyone gets the shared `feeds` plus
their own, with their own read and starred state, devices and pairings. Nobody
sees another user's feeds or state, and a URL several people follow is fetched
once.

```nix
{
  services.pressbox.users = {
    chris = {
      admin = true; # can approve devices for anyone
      feeds.hn = "https://news.ycombinator.com/rss";
    };
    anna.feeds.recipes = "https://example.com/recipes.xml";
  };
}
```

Without `users` there is a single admin user named `default`. When you first
declare users, the existing data (read state, stars, devices) goes to the first
admin. Removing a user deletes their devices and state. Users come only from the
config; nobody signs up.

Moving from another reader? Export OPML from it and run
`pressboxctl opml-to-nix subscriptions.opml > feeds.nix`, then
`services.pressbox.feeds = import ./feeds.nix;`. To add a site whose feed URL
you don't know, `pressboxctl discover https://example.org` finds its feeds and
prints them as Nix.

Some feeds carry only a summary or the first paragraph. For those, set
`fullText = true` on the feed: pressbox fetches each new item's page and keeps
its main content (the article, without navigation, ads or comments), sanitized
like feed HTML. Clients show it in place of the feed's text, and search covers
it. Pages without enough text to be an article keep the feed's own text.

Feed ids (the attribute names) may contain letters, digits, `-` and `_`. The
generated config is checked by `pressboxd --check` at build time, so a bad feed
fails `nixos-rebuild`, not the service.

Removing a feed (for everyone, or from one user's `feeds`) deletes its items for
those users, except starred ones: the feed stays under "removed" in their
clients until they unstar them. Adding it back resumes it.

| Option | Default | Notes |
| ------------------ | ----------- | -------------------------------------------------------- |
| `address` | `127.0.0.1` | `0.0.0.0` or `::` to listen on all interfaces |
| `port` | `8470` | |
| `openFirewall` | `false` | |
| `refreshInterval` | `1800` | Seconds, 60–86400; feeds can set their own |
| `maxItemsPerFeed` | `500` | Starred items are never pruned |
| `maxAgeDays` | `null` | Remove items older than this (starred ones stay) |
| `behindProxy` | `false` | Behind a reverse proxy or `tailscale serve`; see Pairing |
| `backup.enable` | `true` | Daily copy to `/var/lib/private/pressbox/backups` |
| `backup.schedule` | `daily` | systemd OnCalendar |
| `backup.keep` | `14` | |
| `watchdogSec` | `60` | 0 turns the watchdog off |
| `socketActivation` | `false` | Feeds refresh only once the first client connects |
| `users` | `{}` | `<name>.admin`, `<name>.feeds`; see Users |
| `tokenFile` | `null` | Optional static token; acts as the first admin |
| `configFile` | read-only | The generated `pressbox.toml`, for inspection |

For the iOS app, serve pressbox over HTTPS. iOS only allows plain HTTP to local
network addresses. `tailscale serve --bg 8470` is the least work; a reverse
proxy with ACME works too.

### Pairing devices

Every client gets its own token, and only its SHA-256 is stored. To pair:

1. On the new client, start pairing: `pressbox pair`, or "Pair this device" in
   the iOS app. It shows a code like `K7W-4QX`.
1. Approve it on the server with `sudo pressboxctl approve K7W-4QX --user anna`
   (`--user` is needed once there is more than one user), or from a client that
   is already paired: `pressbox approve K7W-4QX`, or iOS Settings. A device
   approved from a client goes to that client's user; admins can name another
   (`pressbox approve CODE --user anna`, or the user field in iOS Settings).
1. The client picks up its token on its own. Codes expire after ten minutes.

`pressboxctl devices` lists devices with their user and when each was last seen;
`pressboxctl revoke <name>` locks one out. For a client that cannot pair (a
script, say), `pressboxctl add-device <name> --user <user>` prints a token.
`pressbox whoami` shows which user and device a machine is signed in as, and the
server's version.

Anyone who can reach the server can ask to pair, so requests are limited: 3
waiting per client address and 50 in all. Behind a reverse proxy or
`tailscale serve`, every request comes from the proxy; set `behindProxy = true`
so the address the proxy puts in `X-Forwarded-For` counts instead. Leave it off
otherwise, since clients could then send any address they like.

### pressboxctl

Runs as root on the server and talks to pressboxd over
`/run/pressbox/control.sock`, which only root and the service can open.

| Command | What it does |
| --------------------------------------- | ---------------------------------------------------------- |
| `status` | Feeds (failing, removed), items, users, devices, pairings |
| `users` | Users, admins, and their feed and device counts |
| `pairings` | Pending pairing requests |
| `approve CODE [--user U]` / `deny CODE` | Answer one |
| `devices [--user U]` | Paired devices, their users, when each was last seen |
| `revoke NAME` | Revoke a device's token |
| `add-device NAME [--user U]` | Create a device and print its token |
| `backup [--keep N]` | Back up now (the timer runs this daily) |
| `refresh` | Fetch every feed now |
| `opml-export [--user U]` | Print a user's feeds as OPML |
| `opml-to-nix FILE` | Convert OPML to a `feeds` attrset (offline) |
| `discover URL` | Find a site's feeds and print them as Nix (no root needed) |

`--user` is required when there is more than one user.

### How feeds are fetched

Each feed has its own schedule. Fetches are conditional (ETag and
Last-Modified), and a server's `Cache-Control: max-age` can stretch the wait but
never shorten it below the interval. A failing feed backs off: the delay doubles
with each failure, up to a day, and `Retry-After` on a 429 or 503 is honored.
`pressboxctl status` and the clients show failing feeds.

Feed HTML is cleaned when it is stored: scripts, styles, iframes, forms and
event handlers are dropped, links get `rel="noopener noreferrer"`, relative URLs
are made absolute, and tracking pixels (1×1 images and known tracker hosts) are
removed. Every item's text is indexed for full-text search.

Each feed keeps its newest `maxItemsPerFeed` items. With `maxAgeDays` set, items
published longer ago are removed too, and not stored when a feed still lists
them. Starred items are kept either way.

### systemd

pressboxd integrates with systemd directly:

- **Readiness and status.** It is a `Type=notify` service, and
  `systemctl status pressbox` shows its own status line, e.g.
  `12 feeds, 2 users, 3 new at 14:30, 1 failing`.
- **Watchdog.** It pings systemd's watchdog only while its database answers, so
  a wedged server is restarted automatically.
- **Credentials.** `tokenFile`, if set, reaches it through `LoadCredential`, so
  the secret can stay readable by root only.
- **Journal.** Logs go straight to the journal with proper priorities:
  `journalctl -t pressboxd -p warning`.
- **Backups.** `pressbox-backup.timer` runs `pressboxctl backup` on schedule.
- **Socket activation (optional).** With `socketActivation = true`, systemd owns
  the port, so it stays open across restarts.

Outside systemd all of this is skipped and pressboxd runs the same by hand.

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
`notifications`) are in Birdland's README.

## iOS

The iOS app is in its own repo,
[pressbox-ios](https://github.com/chrisppy/pressbox-ios), with its own releases.
It needs iOS 18 and talks to the server through API v1; see its README for
building it and what it can do.

## API

The API is versioned: everything lives under `/api/v1/`, except `/health`.
Pairing routes and `version` need no token (pairing is how a client gets one);
every other route needs `Authorization: Bearer <token>`, a paired device's token
or the static `tokenFile` token. A token belongs to one user, and every answer
is about that user's feeds and state. Timestamps are Unix seconds.

| Method | Path | Body / query | Response |
| ------ | --------------------------------- | ---------------------------------------------------------- | ------------------------------------------------------- |
| GET | `/health` | | `ok` |
| GET | `/api/v1/version` | | `{version, api}`, e.g. `{"version": "0.1.0", "api": 1}` |
| POST | `/api/v1/pair` | `{"name": str}` | `{code, poll_id, expires_in}`; 429 when too many wait |
| GET | `/api/v1/pair/{poll_id}` | | 202 pending; `{token, name}` once; 404 |
| GET | `/api/v1/me` | | `{user, admin, device}` |
| GET | `/api/v1/feeds` | | `[Feed]` |
| GET | `/api/v1/items` | `feed`, `tag`, `unread`, `starred`, `q`, `limit`, `offset` | `{items, next_offset}` |
| GET | `/api/v1/items/{id}` | | `Item` with `content` |
| PUT | `/api/v1/items/{id}/read` | `{"value": bool}` | 204 |
| PUT | `/api/v1/items/{id}/starred` | `{"value": bool}` | 204 |
| POST | `/api/v1/feeds/{id}/read-all` | | `{"updated": n}` |
| POST | `/api/v1/refresh` | | 202 |
| GET | `/api/v1/pairings` | | `[{code, name, requested}]` |
| POST | `/api/v1/pairings/{code}/approve` | optional `{"user": str}` (admins only) | `{name, user}` |
| DELETE | `/api/v1/pairings/{code}` | | 204 |

`q` matches every word as a prefix, in titles and text. With `q`, `unread` is
usually left out so read items are found too. Breaking changes will go to a new
version (`/api/v2/`), served alongside `v1` for a while.

## Project layout

```
pressbox/
├── crates/                        the server side
│   ├── pressboxd/                 server and pressboxctl (AGPL-3.0-only)
│   │   └── src/
│   │       ├── main.rs            startup, scheduler, status line
│   │       ├── config.rs          pressbox.toml: feeds, users, paths
│   │       ├── db.rs              SQLite schema, migrations, queries
│   │       ├── fetch.rs           polling, backoff, parsing, full articles
│   │       ├── sanitize.rs        HTML cleaning, search text
│   │       ├── api.rs             HTTP API for clients (/api/v1)
│   │       ├── admin.rs           control-socket API for pressboxctl
│   │       ├── pairing.rs         device tokens and pairing codes
│   │       ├── systemd.rs         notify, watchdog, sockets, journal
│   │       └── bin/pressboxctl.rs admin CLI, feed discovery, OPML
│   └── pressbox-api/              JSON types shared with the TUI
├── clients/
│   ├── tui/                       pressbox, terminal client
│   │   └── src/
│   │       ├── main.rs            CLI: run, pair, approve, whoami
│   │       ├── client.rs          HTTP client, client.json, token
│   │       ├── app.rs             state and keys (tested without a terminal)
│   │       └── ui.rs              drawing
├── modules/
│   ├── nixos.nix                  services.pressbox (AGPL-3.0-only)
│   └── home-manager.nix           programs.pressbox
├── nix/                           flake-parts modules, loaded by devfmt
│   ├── packages.nix               pressboxd, pressbox
│   ├── modules.nix                exports the NixOS and Home Manager modules
│   ├── checks.nix                 module checks, NixOS VM test, reuse lint
│   └── devshell.nix               nix develop
├── assets/                        logo and app icon (CC-BY-SA-4.0)
├── LICENSES/                      license texts (REUSE)
├── .github/workflows/             flake check, cargo-deny, lock updates
├── flake.nix, Cargo.toml, REUSE.toml
├── deny.toml                      allowed licenses and sources (cargo-deny)
├── CHANGELOG.md
└── README.md
```

Files in `nix/` must be flake-parts modules, because devfmt imports every file
there; that is why the NixOS and Home Manager modules live in `modules/`.

## Development

```sh
nix develop          # Rust toolchain, treefmt
cargo test
nix fmt              # devfmt's treefmt setup
nix flake check      # builds, module checks and a NixOS VM test
cargo deny check     # advisories, licenses, duplicate crates, sources
```

`cargo deny` also runs in CI on every push and weekly, so a new advisory against
a dependency shows up without a code change. Its allowed licenses are in
`deny.toml`; anything else needs a deliberate edit there.

Changes are recorded in [CHANGELOG.md](CHANGELOG.md). A release bumps `version`
in `Cargo.toml`, moves the changelog's entries under the new version, and is
tagged `vX.Y.Z`.

To run the server locally, write a `pressbox.toml` (same keys as the module
generates: `listen`, `database`, `refresh_interval`, `max_items_per_feed`,
`max_age_days`, `behind_proxy`, `control_socket`, `backup_dir`, `token_file`,
`[feeds.<id>]` with `url`, `title`, `tags`, `refresh_interval`, `full_text`, and
`[users.<name>]` with `admin` and `[users.<name>.feeds.<id>]`) and run
`cargo run -p pressboxd -- -c pressbox.toml`. The control socket defaults to
`pressbox.sock` next to the database: `pressboxctl -s ./pressbox.sock status`.

## License

pressbox is [REUSE](https://reuse.software) compliant; every file states its
license in an SPDX header (`reuse lint` checks this, also in `nix flake check`).

- The server, `pressboxd` and `pressboxctl` (`crates/pressboxd`), and the NixOS
  module are licensed under the
  [GNU Affero General Public License v3.0 only](LICENSES/AGPL-3.0-only.txt). If
  you let others use a modified server over a network, you must offer them its
  source.
- The terminal client (`pressbox`), the shared `pressbox-api` types, the Home
  Manager module, and the build files and docs are licensed under the
  [GNU General Public License v3.0 only](LICENSES/GPL-3.0-only.txt).
- The logo (`assets/logo.svg`) is licensed under
  [Creative Commons Attribution-ShareAlike 4.0](LICENSES/CC-BY-SA-4.0.txt).

The iOS app, [pressbox-ios](https://github.com/chrisppy/pressbox-ios), is
licensed separately, under the Mozilla Public License 2.0.

### Name and logo

The pressbox name and logo identify this project. The licenses above cover
copyright, not trademarks: you're welcome to use the logo to refer to pressbox,
but forks and derived projects must use their own name and logo.

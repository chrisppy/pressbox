<!--
SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
SPDX-License-Identifier: GPL-3.0-only
-->

# Changelog

All notable changes to pressbox. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). The server, `pressboxctl` and the
terminal client share one version number; the HTTP API has its own (the `v1` in
`/api/v1`), which only changes on an incompatible change. The iOS app,
[pressbox-ios](https://github.com/chrisppy/pressbox-ios), has its own releases.

## [0.1.0] - 2026-11-07

The first release.

### Server

- pressboxd: feeds and users declared in NixOS (`services.pressbox`), with
  shared feeds plus each user's own, and fully separate read and starred state
  per user. RSS 0.9x/1.0/2.0, Atom and JSON Feed, with conditional requests,
  backoff and sanitized HTML.
- Full articles: `fullText = true` on a feed fetches each new item's page and
  keeps its main content, for feeds that only carry a summary.
- Retention by count (`maxItemsPerFeed`) and age (`maxAgeDays`); starred items
  are always kept.
- Full-text search (SQLite FTS5).
- Devices pair with a short code that a signed-in device or `pressboxctl`
  approves; admins can approve for anyone. Pairing requests are limited per
  client address, with `behindProxy` for reverse proxies.
- HTTP API under `/api/v1`, including `GET /api/v1/version` without a token.
- systemd integration: notify and watchdog, socket activation, credentials,
  journald logging, and a backup timer.
- `pressboxctl`: status, users, devices, pairing, refresh, backups, OPML export,
  `opml-to-nix`, and `discover` to find a site's feeds.

### Clients

- `pressbox`, a terminal reader (ratatui).

### Project

- Licenses: server AGPL-3.0-only, terminal client GPL-3.0-only, logo
  CC-BY-SA-4.0; REUSE compliant.
- Checks: NixOS and Home Manager module tests, a NixOS VM test, treefmt and
  reuse lint; `cargo deny` in CI.

[0.1.0]: https://github.com/chrisppy/pressbox/releases/tag/v0.1.0

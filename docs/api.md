<!--
SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
SPDX-License-Identifier: GPL-3.0-only
-->

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
| GET | `/api/v1/items` | `feed`, `tag`, `unread`, `starred`, `q`, `limit`, `cursor` | `{items, next_cursor}` |
| GET | `/api/v1/items/{id}` | | `Item` with `content` |
| PUT | `/api/v1/items/{id}/read` | `{"value": bool}` | 204 |
| PUT | `/api/v1/items/{id}/starred` | `{"value": bool}` | 204 |
| POST | `/api/v1/feeds/{id}/read-all` | | `{"updated": n}` |
| POST | `/api/v1/refresh` | | 202 |
| GET | `/api/v1/pairings` | | `[{code, name, requested}]` |
| POST | `/api/v1/pairings/{code}/approve` | optional `{"user": str}` (admins only) | `{name, user}` |
| DELETE | `/api/v1/pairings/{code}` | | 204 |

Item lists come newest first. To get the next page, pass the previous page's
`next_cursor` as `cursor` with the same filters; it is absent on the last page.
A cursor is opaque, and a malformed one is a 400. The older `offset` parameter
(answered with `next_offset`) still works, but gets slower the further into a
list it goes.

Responses are compressed (gzip or brotli) when the request's `Accept-Encoding`
allows it.

`q` matches every word as a prefix, in titles and text. With `q`, `unread` is
usually left out so read items are found too. Breaking changes will go to a new
version (`/api/v2/`), served alongside `v1` for a while.

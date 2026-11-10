# Encore Edge

The always-on phone backend. It does what the owner's computer does for the
phone today, so the phone keeps working while that computer is off:

- a realtime WebSocket room per listener, relaying commands and state between
  the phone and the computer;
- a small per-user sync store (playlists, queue/position, match choices);
- private media reads with correct byte ranges;
- audio resolution through `yt-dlp` and a remux-only `ffmpeg`;
- the download matcher's conservative YouTube/SoundCloud scoring;
- uploads for the owner's local files and files the computer fetches.

Node 24, plain ES modules, no build step. The only runtime dependency is `ws`.
Nothing here is deployed by this directory's tests; deployment files live in
[`deploy/`](./deploy/README.md).

## Run

```sh
npm install
EDGE_TOKEN_SECRET="$(openssl rand -hex 32)" \
EDGE_DATA_DIR=./data \
EDGE_ADMIN_TOKEN="$(openssl rand -hex 32)" \
npm start
```

`EDGE_TOKEN_SECRET` is required and must be at least 32 characters. The
process refuses to start without it. State, cache and uploads live under
`EDGE_DATA_DIR`; the cache is disposable, uploads are not.

```sh
npm test        # node:test, no network, fake yt-dlp/ffmpeg scripts
npm run check   # node --check on every source file
```

## Deploy

Production is the `encore-edge` container beside the host's existing Caddy; see
[`deploy/README.md`](./deploy/README.md) for the host layout, `deploy.sh` and
the nightly `upkeep.sh` timer. If Caddy already runs there, append the site
block to its Caddyfile in place (it is a single-file bind mount), validate the
whole file in a throwaway container, then `caddy reload`; never restart Caddy
or any other container.

## Environment

| Variable | Default | Meaning |
| --- | --- | --- |
| `EDGE_TOKEN_SECRET` | required | HMAC-SHA256 key shared with the Vercel group service |
| `EDGE_ADMIN_TOKEN` | empty | separate bearer for `/status`; unset disables `/status` |
| `EDGE_HOST` | `127.0.0.1` | listen address; keep it loopback behind Caddy |
| `EDGE_PORT` / `PORT` | `8787` | listen port |
| `EDGE_DATA_DIR` | `./data` | sync, cache, uploads, temp state |
| `EDGE_ALLOWED_ORIGINS` | `https://encore.senni.dev,http://localhost:5173,http://127.0.0.1:5173` | exact browser origins; native clients send no `Origin` |
| `EDGE_CACHE_MAX_BYTES` | `10737418240` (10 GiB) | rolling cache cap; least-recently-played is evicted |
| `EDGE_UPLOAD_FILE_MAX_BYTES` | `536870912` (512 MiB) | one upload |
| `EDGE_UPLOAD_USER_MAX_BYTES` | `5368709120` (5 GiB) | one user's uploads, never evicted |
| `EDGE_SYNC_USER_MAX_BYTES` | `4194304` (4 MiB) | all three sync documents per user |
| `EDGE_MEDIA_TOKEN_TTL_SECONDS` | `3600` | media URL lifetime |
| `EDGE_MEDIA_IDLE_TIMEOUT_MS` | `120000` | a media reader that stops reading is disconnected |
| `EDGE_WS_RATE_PER_SECOND` | `40` | per-connection message rate |
| `EDGE_WS_RATE_BURST` | `80` | per-connection burst |
| `EDGE_LIVE_BUFFER_MS` | `120000` | replay window |
| `EDGE_LIVE_BUFFER_BYTES` | `1048576` | replay bytes per room, whichever cap is reached first |
| `EDGE_LIVE_ENVELOPE_MAX_BYTES` | `16384` | one `state`/`phone` envelope |
| `EDGE_RESOLVE_CONCURRENCY` | `2` | capped at 2 by the service |
| `EDGE_WARM_QUEUE_MAX` | `4` | prefetches waiting behind foreground resolves |
| `EDGE_RESOLVE_TIMEOUT_MS` | `180000` | one yt-dlp job |
| `EDGE_FFMPEG_TIMEOUT_MS` | `120000` | one remux |
| `EDGE_SEARCH_TIMEOUT_MS` | `45000` | one search |
| `EDGE_YTDLP_BIN` | `yt-dlp` | executable path or name |
| `EDGE_FFMPEG_BIN` | `ffmpeg` | executable path or name |
| `EDGE_VAULT_KEY` | empty | 64 hex characters; unset disables the vault and answers `503` on vault, library, search and home routes |
| `EDGE_SPOTIFY_CACHE_TTL_MS` | `60000` | one Spotify/SoundCloud metadata read |
| `EDGE_SPOTIFY_HOME_CACHE_TTL_MS` | `600000` | a complete Home shelf |
| `EDGE_SPOTIFY_SEARCH_TTL_MS` | `120000` | one search |
| `EDGE_LIBRARY_TIMEOUT_MS` | `15000` | one provider request |
| `EDGE_HOME_ARTIST_LIMIT` | `20` | followed artists read for new releases, 1–50 |
| `EDGE_LOG_LEVEL` | `info` | `debug`, `info`, `warn`, `error`, `silent` |

No secret is ever written unencrypted or logged. The optional sign-in vault is
the one place a credential is written, and only as AES-256-GCM ciphertext.

## Tokens

`e1.<base64url(payload)>.<base64url(HMAC-SHA256(payload))>`

Payload:

```json
{ "v": 1, "sub": "group-user", "role": "phone", "scope": "live",
  "exp": 1760000000, "jti": "0123456789abcdef", "key": "optional-media-key" }
```

- `role` is `phone` or `computer`.
- `scope` is `live`, `media` or `sync`.
- `exp` is Unix seconds. Expired tokens are refused.
- `jti` is 8–64 `[A-Za-z0-9_-]` characters.
- `key` is optional and binds a media token to one media key.
- Verification is constant-time; malformed, expired, wrong-scope and
  wrong-role tokens are all refused.

`src/tokens.mjs` exports `signToken`/`mintToken` and `verifyToken`, with no
dependencies, so a Vercel function mints the identical format:

```js
import { mintToken } from "./tokens.mjs";

const token = mintToken({
  sub: accountId,
  role: "phone",
  scope: "live",
  ttlSeconds: 60,
}, process.env.EDGE_TOKEN_SECRET);
```

Tokens may be sent as `Authorization: Bearer …`, `X-Edge-Token: …`, or
`?token=…`. An expired token while a phone is connected is not re-checked;
reconnect with a fresh one.

## HTTP API

All JSON responses carry `v: 1` on success. Errors are
`{ "v": 1, "error": "code" }` with the status code; input is never echoed.

| Route | Auth | Purpose |
| --- | --- | --- |
| `GET /health` | none | `{"v":1,"ok":true}` |
| `GET /status` | admin | disk, cache, uploads, queue, connections, sync users |
| `GET/PUT /v1/sync/playlists` | sync | playlist snapshot, mixed playlists included |
| `GET/PUT /v1/sync/queue` | sync | queue and position record |
| `GET/PUT /v1/sync/matches` | sync | Spotify URI → source choice |
| `PUT/DELETE /v1/vault/spotify` | sync, computer | store or remove a Spotify refresh token and its Client ID |
| `PUT/DELETE /v1/vault/soundcloud` | sync, computer | store or remove a SoundCloud OAuth token |
| `GET /v1/library/playlists` | sync, phone | saved Spotify playlists plus mixed playlists from the sync snapshot |
| `GET /v1/library/playlists/:id` | sync, phone | one playlist and its songs |
| `GET /v1/library/liked` | sync, phone | liked songs |
| `GET /v1/library/albums` | sync, phone | saved albums |
| `GET /v1/library/artists` | sync, phone | followed artists |
| `GET /v1/library/soundcloud/likes` | sync, phone | SoundCloud likes, when a token is stored |
| `GET /v1/library/soundcloud/playlists` | sync, phone | SoundCloud own and liked playlists, when a token is stored |
| `GET /v1/search?q=` | sync, phone | Spotify and SoundCloud in one answer |
| `GET /v1/artist/:id` | sync, phone | artist and top tracks |
| `GET /v1/album/:id` | sync, phone | album and songs |
| `GET /v1/home` | sync, phone | recently played and new releases |
| `POST /v1/resolve` | media | resolve a source and return a media URL |
| `POST /v1/warm` | media | prepare at most two upcoming sources |
| `POST /v1/match` | media | score YouTube and SoundCloud search results |
| `POST /v1/media-token` | media | mint a fresh URL for an entry the user owns |
| `GET/HEAD /v1/media/:key` | media | private media with Range support |
| `PUT /v1/upload/:key` | media, computer | stream one audio file in |
| `GET /v1/live` (WebSocket upgrade) | live | realtime relay |

### Sync

`PUT` body is `{ "data": <json>, "revision": <optional int> }`. A supplied
revision must match the stored one or the answer is `409 revision_conflict`.
`GET` returns `{v, doc, revision, updatedAt, data}`; a never-written document
has `revision: 0` and `data: null` (`{}` for matches).

Matches are merged, not blindly replaced. Each entry is:

```json
{ "source": "youtube|soundcloud|local|blob|spotify",
  "sourceUri": "youtube:video:abcdefghijk",
  "confidence": 0.93,
  "date": "2026-11-09T20:00:00.000Z",
  "manual": false }
```

A `manual: true` entry always outranks an automatic one, even when the
automatic answer is newer or more confident. `remove: ["spotify:track:…"]`
deletes automatic entries only; hand-chosen entries are returned in `kept`.

### Sign-in vault, library, search and home

The vault lets the always-on service use a listener's own Spotify and
SoundCloud sign-ins while their computer is off. A listener opts in by having
their computer `PUT` a credential; a phone can never write one.

```json
PUT /v1/vault/spotify     { "client_id": "…", "refresh_token": "…" }
PUT /v1/vault/soundcloud  { "oauth_token": "…" }
→ { "v": 1, "provider": "spotify" }

DELETE /v1/vault/spotify  → { "v": 1, "provider": "spotify", "deleted": true }
```

Both routes need a `computer`/`sync` token. No route returns a token, and no
token is logged. The Spotify Client ID is the listener's own app; the service
never uses anyone else's. `oauth_token` accepts only a token value, not a pasted
cookie or `Authorization:` header.

On disk each user's entry is AES-256-GCM encrypted with `EDGE_VAULT_KEY` and
written atomically. Deleting the last provider removes the file. If
`EDGE_VAULT_KEY` is unset, the vault and every route below answer `503
vault_not_configured`; the rest of the service still runs.

Spotify access uses the PKCE refresh grant with the stored Client ID, without a
client secret. Access tokens stay in memory; a rotated refresh token is written
back atomically before the access token is used. One refresh runs per listener
at a time, a `429` response pauses that listener until its `Retry-After` has
passed (also sent to the phone), and successful reads are cached briefly so
reopening a screen spends no quota.

All library routes need a `phone`/`sync` token, accept `limit` from 1 to 50
(default 50) and an opaque `cursor`, and return:

```json
{ "v": 1, "items": [ … ], "next": "opaque-cursor-or-null" }
```

Search pages are capped at 10 results per source, because a personal Spotify
app in Development Mode refuses a larger page, and its cursor is shared by both
sources. A compact track is:

```json
{ "source": "spotify", "uri": "spotify:track:…", "name": "…",
  "artists": [{ "id": "…", "name": "…" }],
  "album": { "id": "…", "name": "…" },
  "image": "https://…", "duration_ms": 200000 }
```

A compact album carries `id`, `uri`, `name`, `image`, `artists`, `release_date`,
`total_tracks` and `album_type`. An artist carries `id`, `uri`, `name`, `image`,
`genres` and `followers`. A playlist carries `id`, `uri`, `name`, `image`,
`owner`, `track_count` and `collaborative`.

- `GET /v1/library/playlists` returns Spotify playlists in Spotify's order and,
  on the first page only, the mixed playlists from the phone's sync snapshot.
  A mixed row has `source: "mixed"`, its snapshot `id`, `uri`, `name`, `image`
  and `track_count`. If Spotify is not linked, the first page still returns the
  mixed rows with `"error": "spotify_not_linked"`.
- `GET /v1/library/playlists/:id` returns `{v, playlist, items, next}`. An id
  that is not a Spotify playlist falls back to the same mixed snapshot.
- `GET /v1/library/liked` and `/v1/library/albums` return Spotify saved items.
- `GET /v1/library/artists` pages through followed artists with a `cursor`
  carrying Spotify's `after` value.
- `GET /v1/library/soundcloud/likes` and
  `/v1/library/soundcloud/playlists` return SoundCloud rows when the listener
  stored an OAuth token. Without one they answer `409 soundcloud_not_linked`.
- `GET /v1/search?q=` answers both sources and never lets one failure hide the
  other:

  ```json
  { "v": 1, "query": "…", "next": null,
    "spotify": { "tracks": [], "artists": [], "albums": [], "playlists": [],
                 "error": "spotify_rate_limited" },
    "soundcloud": { "tracks": [], "playlists": [],
                    "error": "soundcloud_unavailable" } }
  ```

  An `error` field appears only for a source that failed; the other source's
  rows are still there.
- `GET /v1/artist/:id` returns `{v, artist, top_tracks}`.
- `GET /v1/album/:id` returns `{v, album, items, next}`.
- `GET /v1/home` returns the first 50 recently played tracks (deduplicated,
  each with `played_at`) and up to 50 new albums and singles from followed
  artists, newest first. It is cached for `EDGE_SPOTIFY_HOME_CACHE_TTL_MS` only
  when both sections succeed:

  ```json
  { "v": 1, "recently_played": [ … ], "new_releases": [ … ],
    "generated_at": "…", "errors": { "recently_played": "spotify_rate_limited" } }
  ```

New releases use the Web API fallback the desktop app describes: followed
artists, then each artist's latest albums and singles. `GET /browse/new-releases`
is not that feed, `/recommendations`, related artists and audio features are
refused to apps created since November 2024, and no private Pathfinder request
is made here.

SoundCloud metadata follows `src/soundcloud/` in the Rust app:
`api-v2.soundcloud.com` search and user routes, the public client ID discovered
from `soundcloud.com`'s script bundles, and `Authorization: OAuth …` only for
the listener's own likes and playlists. Public search needs no account. A stale
public client ID is refreshed once. Audio is never requested.

### Resolve, warm, match

```json
POST /v1/resolve  { "source": "youtube:video:abcdefghijk" }
→ { "v":1, "key":"c…", "contentType":"audio/mp4", "size":1234,
    "cached":false, "url":"/v1/media/c…?token=…" }

POST /v1/warm     { "sources": ["youtube:video:…", "soundcloud:artist/track"] }
→ 202 { "v":1, "warmed":[{"key":"c…","state":"queued"}], "rejected":0 }

POST /v1/match    { "uri":"spotify:track:…", "name":"…", "durationMs":200000,
                    "artists":[{"name":"…"}] }
→ { "v":1, "match": { "source":"soundcloud:artist/track", "provider":"soundcloud",
                      "score":0.98, "title":"…", "uploader":"…", "duration":200.3 } }
→ { "v":1, "match": null }        // below the conservative threshold
```

Only these sources are accepted: `youtube:video:<11 chars>`,
`soundcloud:<artist>/<track>`, or an `https://soundcloud.com/...` URL with no
query or fragment. Everything else is `400 invalid_source` before a child
process exists. `yt-dlp` is invoked with an argument array, `--` before the
URL, and no shell. It writes the chosen stream to stdout; `ffmpeg` remuxes it
with `-c:a copy` (never transcoding) into a fragmented M4A, or MP3/Ogg/FLAC when
the source is that format. At most two jobs run; identical requests share one
job.

`POST /v1/resolve` answers as soon as the first bytes exist, with
`"streaming": true` and a media URL that is already playable. The download keeps
running while the phone reads. `cached: true` and `"streaming": false` come from
a second resolve after the file is complete. A client that disconnects before
the first byte kills its tools and frees the slot; a prefetch (`warm`) never
holds the slot against a real tap and is dropped once `EDGE_WARM_QUEUE_MAX` is
reached.

### Media

`GET/HEAD /v1/media/:key?token=…`

- single ranges (`bytes=0-9`), open-ended (`bytes=10-`) and suffixes
  (`bytes=-4`) are supported;
- multiple ranges, reversed ranges and ranges beyond the end answer `416`
  with `Content-Range: bytes */size`;
- a file that is still being fetched is served from its `.part` file: a whole
  request is chunked with no `Content-Length`, and a single range waits for the
  bytes it asked for and answers `206` (with `/*` while the total is unknown);
- `Accept-Ranges: bytes`, `Content-Length`, a strong `ETag`, no
  `Content-Encoding`, `Cache-Control: private, max-age=3600`;
- CORS names only a configured origin and exposes `Content-Range`,
  `Accept-Ranges`, `Content-Length` and `ETag`;
- the token must match both the user and the exact key;
- a reader pins its cache entry, so eviction never deletes a file mid-stream;
  a reader that stops reading is disconnected after
  `EDGE_MEDIA_IDLE_TIMEOUT_MS`.

### Upload

`PUT /v1/upload/:key` with a media token for the `computer` role. The body is
streamed to a temp file, checked against the file and per-user caps, checked
for audio magic (MP4/M4A, MP3/ID3, Ogg, FLAC, WAV, AIFF), fsynced and renamed.
A successful upload answers `201` with `key`, `size`, `contentType`, `etag`
and a media `url`. Uploads are stored apart from the cache and are never
evicted. If the token contains a `key` claim it must equal the route key.

## Live wire protocol

Connect to `/v1/live?token=…` (or `Authorization: Bearer …`) with
`since=<last-seq>` to replay immediately after the handshake. Browser
connections must send an allowed `Origin`. A native client may omit it.

Every envelope is JSON:

```json
{ "v": 1, "t": "command", "id": "client-chosen-id", "...": "payload" }
```

The server adds `seq` and `from` (`"phone"` or `"computer"`) to every message
it relays or buffers. `from` is not trusted from the client.

| `t` | Direction | Meaning |
| --- | --- | --- |
| `hello` | both | handshake / resume request |
| `command` | phone → computer | a playback or library command |
| `state` | both | latest playback state for the sender's role |
| `reply` | both | answer to a command |
| `phone` | phone | phone-specific state/notice |
| `control` | both | small control word |
| `ping` | both | application-level keepalive |
| `pong` | both | answer to `ping` |

The server sends an unsolicited `hello` on connect:

```json
{ "v":1, "t":"hello", "id":"srv-…", "ok":true,
  "role":"phone", "sub":"…", "seq":12, "epoch":"…",
  "reset":false, "replayed":0,
  "state": { "t":"state", "seq":11, "body": { … } },
  "heartbeatMs":15000, "bufferMs":120000 }
```

- `seq` is assigned by the server to every relayed/stored message and grows
  per user. Clients remember the last `seq` they saw.
- On reconnect, send `hello` with `since: <lastSeq>` (or put `since=` in the
  URL). Every buffered message after it is replayed before normal traffic.
  `reset: true` means the server restarted, the client is ahead, or the byte
  cap (`EDGE_LIVE_BUFFER_BYTES`) forced the oldest messages out: reload
  authoritative data from `/v1/sync/*`.
- Messages from one role are relayed only to the other role's connections.
  `hello`, `ping` and `pong` are not relayed.
- `state` and `phone` update the latest state for the sender's role. A new
  connection receives the other role's latest state in `hello.state`.
- A `command` id is deduplicated per user for five minutes. A duplicate is
  dropped and answered to its sender with
  `{"t":"reply","id":"…","ok":true,"duplicate":true}`.
- Bad input answers
  `{"v":1,"t":"reply","id":"…","ok":false,"error":"invalid_json"}` (or
  `invalid_envelope`, `invalid_id`, `rate_limited`, `binary_not_supported`).
  It never closes the process. A frame over 64 KiB closes only that socket,
  and a `state`/`phone` body over `EDGE_LIVE_ENVELOPE_MAX_BYTES` is refused.
- Native WebSocket pings run every 15 s; a client that misses one is
  terminated. Application `ping` is answered with `pong`.
- Each connection has a token bucket (`EDGE_WS_RATE_PER_SECOND` /
  `EDGE_WS_RATE_BURST`). Persistent abuse closes with code `1008`.

## Storage

```
$EDGE_DATA_DIR/
  sync/<user-hash>/playlists.json
  sync/<user-hash>/queue.json
  sync/<user-hash>/matches.json
  vault/<user-hash>.json
  cache/<key>.m4a|mp3|ogg|flac|wav
  cache/<key>.<ext>.part        (a fetch in progress; swept on start)
  cache/last-played.json
  uploads/<owner-prefix>/<key>.<ext>
```

Every JSON and every cache index is written to a temp file, fsynced, then
renamed. A cache file only gets its final name after the bytes are fsynced; a
crash leaves a `.part` file that is removed on start. Deleted or corrupt cache
files are rebuilt from the directory on start; sync documents are loaded lazily
from disk, and a corrupt one is kept as `.corrupt-<time>` and answered with
`500 sync_corrupt` rather than silently reset to revision 0. Uploads are
indexed from disk on start.

## Logging

Log lines are JSON with an event name and small fields: method, route
template, status, duration, counts, error codes. Request URLs, query strings,
tokens, admin tokens, source links, media keys and user file names are never
logged. Child-process stderr is drained but never read into a response or a
log line.

## Not done

- Resolution does not start serving bytes before the tool finishes.
- A SoundCloud URL with a query string (for example a private token) is
  refused by the strict allowlist.
- There is no HTTP endpoint that mints upload tokens; the computer sends a
  `computer`/`media` token and the key is fixed by the route.
- The vault has no key rotation: changing `EDGE_VAULT_KEY` makes old entries
  unreadable and the listener must connect again.
- Library routes only read. Saving, liking, playlist edits and playback control
  stay on the computer.
- A personal Spotify app in Development Mode answers at most ten results per
  search type, so search pages are ten. Playlists Spotify withholds from a
  personal app, and related artists, recommendations, audio features and the
  private Pathfinder feed are not requested; Home's new releases use followed
  artists and their albums instead.
- SoundCloud likes and playlists need a stored OAuth token and are not refreshed
  by the service. SoundCloud audio is never resolved here.
- Home reads up to `EDGE_HOME_ARTIST_LIMIT` followed artists, one album request
  each. A rate limit keeps the albums already read and leaves the section
  partial instead of caching it.

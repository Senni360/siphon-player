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
| `EDGE_WS_RATE_PER_SECOND` | `40` | per-connection message rate |
| `EDGE_WS_RATE_BURST` | `80` | per-connection burst |
| `EDGE_LIVE_BUFFER_MS` | `120000` | replay window |
| `EDGE_RESOLVE_CONCURRENCY` | `2` | capped at 2 by the service |
| `EDGE_RESOLVE_TIMEOUT_MS` | `180000` | one yt-dlp job |
| `EDGE_FFMPEG_TIMEOUT_MS` | `120000` | one remux |
| `EDGE_SEARCH_TIMEOUT_MS` | `45000` | one search |
| `EDGE_YTDLP_BIN` | `yt-dlp` | executable path or name |
| `EDGE_FFMPEG_BIN` | `ffmpeg` | executable path or name |
| `EDGE_LOG_LEVEL` | `info` | `debug`, `info`, `warn`, `error`, `silent` |

No secret is ever written to disk or logged.

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
URL, and no shell. The result is remuxed with `ffmpeg -c:a copy` to M4A, or to
MP3 when the source was MP3. It is never transcoded. At most two jobs run;
identical requests share one job.

The first byte waits for the whole job: the resolver does not stream a
partially downloaded file. `warm` still hides most of that delay.

### Media

`GET/HEAD /v1/media/:key?token=…`

- single ranges (`bytes=0-9`), open-ended (`bytes=10-`) and suffixes
  (`bytes=-4`) are supported;
- multiple ranges, reversed ranges and ranges beyond the end answer `416`
  with `Content-Range: bytes */size`;
- `Accept-Ranges: bytes`, `Content-Length`, a strong `ETag`, no
  `Content-Encoding`, `Cache-Control: private, max-age=3600`;
- CORS names only a configured origin and exposes `Content-Range`,
  `Accept-Ranges`, `Content-Length` and `ETag`;
- the token must match both the user and the exact key.

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
  `reset: true` means the server restarted or the client is ahead: reload
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
  It never closes the process. A frame over 64 KiB closes only that socket.
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
  cache/<key>.m4a|mp3|ogg|flac|wav
  cache/last-played.json
  uploads/<owner-prefix>/<key>.<ext>
```

Every JSON and every cache index is written to a temp file, fsynced, then
renamed. Deleted or corrupt cache files are rebuilt from the directory on
start; sync documents are loaded lazily from disk. Uploads are indexed from
disk on start.

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

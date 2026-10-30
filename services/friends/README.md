# Siphon friends

Private group feed for Siphon Player. A dependency-free Node service in TypeScript,
with Vercel Node Functions and an optional Upstash Redis REST adapter. TypeScript,
Node types and tsx are development tools only. Nothing has been deployed.

## Local development

Use Node 24 and npm. From this directory:

```powershell
npm ci
$env:SIPHON_FRIENDS_INVITE_CODE = 'siphon-local-test-invite'
npm run dev
```

The runner binds only `127.0.0.1:8787`. Set `PORT` to change it. Without Redis,
local development uses in-memory storage which disappears on restart.

```powershell
npm run check
npm test
```

To use the local service in the player, set
`SIPHON_FRIENDS_URL=http://127.0.0.1:8787/api` in the player's environment.
Use a disposable demo profile. The default URL in `src/friends/client.rs` is an
intentional `.invalid` placeholder; the client reports that configuration is
missing without contacting it. Configure the real deployment before use.

The opt-in Rust integration test talks to this runner without touching any app
profile, credential store, or playback process. From the repository root:

```powershell
cargo test --locked --features demo friends::tests::local_service_round_trip -- --ignored --exact
```

## Deployment (orchestrator only)

1. Create a Vercel project with **Root Directory `services/friends`**, Framework
   Preset **Other**, Node.js **24.x**, no build command and no output directory.
   `api/[...path].ts` is the Node Function for all `/api/*` routes.
2. Connect an Upstash Redis database through the Vercel integration. Confirm the
   project environment contains a matching REST URL and read/write token pair.
3. Set `SIPHON_FRIENDS_INVITE_CODE` to a randomly generated group invite (12 to
   256 characters, no control characters). Keep it in Vercel environment settings,
   never in git. The same invite joins the single group at this deployment.
4. Deploy, then set the player's `SIPHON_FRIENDS_URL` to the deployed HTTPS URL
   including `/api` (or replace `DEFAULT_BASE_URL` for a distributed build).
5. Verify join, a post, a heart, a rename, own-post deletion and membership
   revocation. Verify posts survive a redeployment. Rotate the invite if it
   escapes the group.

Environment variables:

| Variable | Purpose |
| --- | --- |
| `SIPHON_FRIENDS_INVITE_CODE` | Required group invite. Minimum 12 characters. |
| `KV_REST_API_URL`, `KV_REST_API_TOKEN` | Vercel integration's Redis REST endpoint and read/write token. |
| `UPSTASH_REDIS_REST_URL`, `UPSTASH_REDIS_REST_TOKEN` | Alternative Redis REST credentials. KV names take precedence. |
| `PORT` | Local runner port, default 8787. |
| `SIPHON_FRIENDS_URL` | **Player-side only.** Base service URL including `/api`. |

Production refuses to start without both Redis values. It never falls back to
ephemeral memory on Vercel or with `NODE_ENV=production`. Keep Vercel deployment
protection disabled for the API's production hostname, or the native client will
receive a login page. The API itself authenticates every feed request.
If either `KV_REST_API_*` value is supplied, both must be supplied. The adapter
never pairs a KV URL with an Upstash token (or the reverse). Missing/malformed
Redis replies fail closed; only an explicit Redis `null` means an empty group.

## API

All bodies and responses are JSON. Except join, requests require
`Authorization: Bearer <secret>`. Secrets are 32 random bytes encoded as 64 hex
characters. The join response is the only server response containing the secret;
only SHA-256 hashes are stored. All responses are `Cache-Control: no-store`.
Never log request bodies, Authorization headers, invites, or private share URLs.

| Method and route | Body / result |
| --- | --- |
| `POST /api/join` | `{invite_code, display_name}` -> `201 {user_id, secret}` |
| `GET /api/me` | `{user_id, display_name}` |
| `PATCH /api/me` | `{display_name}` -> profile. Renames also appear on old posts. |
| `DELETE /api/me` | Revoke membership and its bearer secret -> `{ok:true}`. Shared posts remain attributed to `Former member`; this member's hearts are removed. |
| `POST /api/posts` | Post payload -> `201` post with author and heart fields. |
| `GET /api/posts?limit=30&cursor=…` | `{posts, next_cursor}`. Newest first; limit 1 to 100. Cursor is opaque. |
| `DELETE /api/posts/:id` | Delete own post -> `{ok:true}`. Other authors receive 403. |
| `PUT /api/posts/:id/like` | Set heart, idempotent -> `{ok:true}`. |
| `DELETE /api/posts/:id/like` | Remove heart, idempotent -> `{ok:true}`. |

Payload: `{url, source, title, artists, artwork_url, duration_ms, note?}`.
`source` is `spotify`, `soundcloud` or `youtube`; `artists` is an array of names,
`artwork_url` may be null, and `duration_ms` is an integer (zero means unknown).
Post fields also include `id`, `author_id`, `author_name`, `created_at` (Unix
milliseconds), `like_count`, and `liked` for the authenticated reader.

Only HTTPS links from the requested source are accepted: Spotify tracks, albums
and playlists; SoundCloud tracks and sets, with private `s-` paths/query tokens
preserved; `on.soundcloud.com` links; YouTube watch and youtu.be links with an
11-character video ID. No links are fetched by the service, including artwork.
Sharing a private SoundCloud link grants everyone in the group access to that
link. It is intentionally stored with its secret token.

Names: 1 to 40 characters; title: 1 to 300; up to 20 artist names (200 each);
note: up to 280; request: up to 16 KiB. No control characters. The service limits
join attempts to 10/hour/IP, all requests to 180/minute/IP and authenticated
requests to 120 reads or 60 writes/minute/user. On Vercel the IP comes from the
platform's overwritten `x-vercel-forwarded-for`, not client-supplied forwarding
headers. IP identifiers are hashed in the rate-limit keys. 429 includes
`Retry-After: 60` for minute budgets or `3600` for the hourly join budget. These
are conservative window bounds; rejected attempts do not extend a window.
The client uses longer exponential backoff after feed failures.

Storage uses atomic compare-and-set on a single versioned group document and
expiring rate counters, under `siphon:friends:v1:*`. This deliberately favors
the owner's small group over an indexed public social network. Concurrent writes
retry only explicit storage CAS conflicts, with up to 20 attempts and jittered
backoff capped at 64 ms. An uncertain transport failure is never retried, because
its write may already have committed. Requests have a 12-second I/O deadline;
each Redis REST command is limited to five seconds. Client disconnects cancel
pending body reads, storage requests and conflict waits. Cancellation cannot
undo a write already accepted by Redis.

The group is capped at 500 identities, 5,000 posts and 8 MiB of serialized state,
whichever is reached first. A write exceeding the byte budget returns 409
without replacing the existing document. Redis response reads are also bounded,
including chunked bodies and JSON escaping overhead. Persisted row schemas,
source URLs, identity uniqueness and heart membership are checked before use;
malformed data fails closed rather than silently becoming an empty feed. Feed
responses contain only their documented public fields. Delete old posts before
the feed fills. Back up Redis
and monitor its quota. There is no admin UI or token recovery endpoint. Former
members' retained posts need administrator storage maintenance if they must be
removed later. Delete your posts before leaving if you want them removed.

Before upgrading an existing deployment or importing a group, check and back up
the serialized Redis state. An existing document above 8 MiB is rejected on read,
so API deletion cannot reduce it. The owner must archive or prune that document
directly in storage before this version can serve it. No data is silently
discarded or automatically migrated. No live group was accessed during testing.

**Leave group** calls `DELETE /me`, revokes the secret and then removes the
device credential. Previously shared private SoundCloud links remain visible
to the group. All mutations recheck membership inside each compare-and-set
attempt, so a request authenticated before leaving cannot commit afterward.
**Forget this device** removes only the local credential and works offline;
it does not revoke the server membership or remove posts. A failed online leave
keeps the protected identity for an explicit retry. Neither writes nor joins
are automatically retried after an uncertain network response.

## Player integration API

`src/friends/` owns `Client`, `NewPost`, `Post`, `Feed`, `Profile`, `Model`,
`Action`, `Request`, `Event`, and the ordered backend `Runtime`. The redesign
should read `App::friends_feed` and emit `model::Action::Friends(...)`:

- `Open`, `Refresh`, `More`, `Join { invite_code, display_name }`, `Rename(name)`,
  `Leave` (local forget), `LeaveGroup` (remote revocation), `Share(NewPost)`,
  `Like { id, liked }`, `Delete(id)`, `DismissSecret`.
- `NewPost::from_item(&PlayableItem)` maps Spotify tracks, complete SoundCloud
  external links and `youtube:video:<id>` tracks. `NewPost::from_url` supports
  albums, playlists and sets with unknown duration.
- Feed links pass through `link::parse` and the app's existing `OpenLink` action;
  an allowlisted YouTube web URL opens externally until native link handling
  lands from the YouTube worker.
- `Model::begin` applies optimistic writes and blocks overlapping operations.
  Sign-out can interrupt pending work. Every request has a new generation;
  joining or leaving clears the previous feed, cursor and one-time secret.
  `Model::receive` ignores old-generation events. Errors preserve
  existing rows. Refresh is explicit after an uncertain mutation; writes are
  never automatically retried.
- Automatic reads happen only with the panel and window visible, at most once
  per 60 seconds, backing off to 16 minutes. Closing and reopening fetches again.
- Identity bytes use `credentials::Slot::Friends`, with the existing verified
  protected store, Windows sealed fallback and revocation markers. They never
  go into settings or the demo/real Spotify sign-in files. The stored identity
  is bound to its service base URL, preventing a dev override from leaking a
  production secret to another server. Network operations use the configured
  proxy, a deadline, no redirects and no automatic transport retries.

## Validation coverage

`npm test` exercises in-memory lifecycle/authorization, private-link validation,
pagination, limits, concurrent mutations and membership revocation races. A
local HTTP test runs the real Node adapter with streamed and pre-parsed Buffer
bodies through join/share/heart/rename/delete/leave. Redis REST tests use an
isolated fake transport, including two service instances sharing a simulated
persisted document, CAS conflicts, expiration commands, invalid replies and
configuration failures. A bounded six-client burst exercises 48 simultaneous
creates and 18 duplicate heart operations; pagination fixtures combine equal
timestamps with creation, deletion, rename and revocation. Additional cases
cover CAS exhaustion, a committed write whose response is lost, shared
concurrent rate limits, body/Redis cancellation, malformed and oversized Redis
responses, and document capacity. Stress timings printed by the suite describe
only the local fixture, not production latency or capacity. These tests do not
claim a live Upstash or Vercel run.

Rust's `friends::tests` covers the fake-server protocol, secrets redaction,
account-switch generations and cancellation of a stalled join using a disposable
in-memory credential backend. The ignored `local_service_round_trip` runs the
Rust client against the loopback Node service, without app credentials. Native
Windows credential storage and a reviewed production deployment still require
the local coordinator's validation.

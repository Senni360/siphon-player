# Siphon friends

Private group feed for Siphon Player. A dependency-free Node service in TypeScript,
with Vercel Node Functions and an optional Upstash Redis REST adapter. TypeScript,
Node types and tsx are development tools only. It runs as the Vercel project
`siphon-friends`, at `https://siphon-friends.vercel.app/api`.

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
Use a disposable demo profile. Without it, the player uses the deployed group
named by `DEFAULT_BASE_URL` in `src/friends/client.rs`.

The opt-in Rust integration test talks to this runner without touching any app
profile, credential store, or playback process. From the repository root:

```powershell
cargo test --locked --features demo friends::tests::local_service_round_trip -- --ignored --exact
```

## Deployment (orchestrator only)

1. Create a Vercel project with **Root Directory `services/friends`**, Framework
   Preset **Other**, Node.js **24.x**, no build command and no output directory.
   `api/index.ts` is the Node Function; `vercel.json` rewrites every `/api/*`
   route to it. Relative imports name `.js`, which TypeScript resolves to the
   `.ts` sources, because Vercel compiles each file on its own.
2. Connect an Upstash Redis database through the Vercel integration. Confirm the
   project environment contains a matching REST URL and read/write token pair.
3. Set `SIPHON_FRIENDS_INVITE_CODE` to a randomly generated group invite (12 to
   256 characters, no control characters). Keep it in Vercel environment settings,
   never in git. The same invite joins the single group at this deployment.
4. Deploy, then set the player's `SIPHON_FRIENDS_URL` to the deployed HTTPS URL
   including `/api`, or replace `DEFAULT_BASE_URL`, which a distributed build
   uses. Siphon's own group is at `https://siphon-friends.vercel.app/api`.
5. Verify join, a post, a heart, a rename, own-post deletion and membership
   revocation. Verify posts survive a redeployment. Rotate the invite if it
   escapes the group.

Environment variables:

| Variable | Purpose |
| --- | --- |
| `SIPHON_FRIENDS_INVITE_CODE` | Required group invite. Minimum 12 characters. |
| `KV_REST_API_URL`, `KV_REST_API_TOKEN` | Vercel integration's Redis REST endpoint and read/write token. |
| `UPSTASH_REDIS_REST_URL`, `UPSTASH_REDIS_REST_TOKEN` | Alternative Redis REST credentials. KV names take precedence. |
| `TYPESAFE_API_KEY` | Optional. TypeSafe key for Jev; the service calls `api.typesafe.ai` with it. |
| `AI_GATEWAY_API_KEY` | Optional. Without a TypeSafe key, Jev goes through Vercel AI Gateway with this key, or with the deployment's own OIDC token. |
| `PORT` | Local runner port, default 8787. |
| `SIPHON_FRIENDS_OWNER_SPOTIFY_ID` | Optional. The Spotify username that owns the first group. That account takes it over when it next connects. |
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
| `GET /api/members` | `{members: [{user_id, display_name}]}`, for choosing who a share goes to. |
| `POST /api/posts/:id/replies` | `{text}` (1 to 500 characters) -> `201 {id, author_id, author_name, created_at, text}`. |
| `DELETE /api/posts/:id/replies/:reply` | Delete own reply -> `{ok:true}`. Other authors receive 403. |
| `POST /api/jev/next` | `{recent: [{title, artists}] (1 to 10), candidates: [{id, title, artists}] (2 to 255)}` -> `{probabilities: {id: p} \| null}`. |
| `POST /api/jev/titles` | `{uploads: [{title, uploader}] (1 to 50)}` -> `{readings: [{artist, title} \| null]}`, in order. |

### Accounts and groups

An account is a Spotify account: `POST /api/auth/spotify` takes `{token, device}`,
reads the id with `GET https://api.spotify.com/v1/me`, and answers `201 {user_id,
display_name, secret, created}`. The token is used for that one read and is never
stored or logged. Each computer gets its own secret, stored hashed. A bearer sent
along links the identity it names; a second identity of the same person folds into
the account, and their old secret keeps working as one of its devices.

Every feed, live and playlist request is about one group, named by the
`X-Siphon-Group` header. Without it a request means the first group, which holds
everyone from before groups existed, so an older client keeps working unchanged.
A request for a group the reader is not in gets 403.

| Method and route | Body / result |
| --- | --- |
| `GET /api/account` | `{user_id, display_name, spotify_id, soundcloud, devices, groups}`. `invite` shows only to those who may invite; `pending` only to admins. |
| `DELETE /api/account/devices/:id` | Sign a computer out. |
| `PUT` / `DELETE /api/account/soundcloud` | `{id, username}`: a label only. |
| `DELETE /api/account/spotify` | Unlink. New computers cannot find the account until it connects again. |
| `POST /api/groups` | `{name}` -> the group, with the caller as owner and a fresh invite. |
| `POST /api/groups/join` | `{code}` -> the group. 403 for a wrong code, or for someone an admin removed. |
| `PATCH` / `DELETE /api/groups/:id` | Rename (admin); delete (owner). The first group cannot be deleted. |
| `POST` / `DELETE /api/groups/:id/invite` | New code, ending the old one (needs the invite right); turn codes off (admin). |
| `POST /api/groups/:id/leave` | The owner's seat falls to the longest-standing admin, else member. The last to leave deletes the group. |
| `POST /api/groups/:id/owner` | `{user_id}`: hand over (owner). |
| `POST` / `DELETE /api/groups/:id/members` | `{spotify_id}`, a username or profile link: add, or cancel someone not signed in yet, who joins when they do (admin). |
| `PATCH /api/groups/:id/members/:user` | `{role?, rights?}`. Only the owner changes admins. |
| `DELETE /api/groups/:id/members/:user` | Remove. Old invites do not bring them back; adding them does. |

Rights are `{share, host, queue, invite}`, enforced here, not just hidden in the
app. Owners and admins may also delete anyone's shares and replies in their group.

A removed member's live access lasts at most the 60 seconds another instance may
remember the roster.

### Shared playlists

| Method and route | Body / result |
| --- | --- |
| `GET /api/playlists` | `{playlists: [{id, name, by, by_name, created_at, count, artwork_url, to}]}`. |
| `POST /api/playlists` | `{name, items (1 to 1000 songs), to?}` -> `201` the row. |
| `GET /api/playlists/:id` | `{id, name, items}`. Reading keeps it for another 400 days. |
| `DELETE /api/playlists/:id` | Its sharer or an admin. |

A share is a copy of the list as it was: later edits do not follow. Songs live in
their own expiring key, so the group's document stays small.

### Listening together

Live state sits in its own expiring Redis keys beside the group's document, so
a live read is one `MGET`. Who belongs is remembered per instance for 60 seconds:
a removed member keeps live access for at most that long, and a known member's
live request costs no document read and no Redis rate-limit call. Each member may
make 120 live reads and 60 live writes a minute per instance.

| Method and route | Body / result |
| --- | --- |
| `GET /api/live` | `{now, presence: [{user_id, item, position_ms, playing, at, following}], channels: [Channel]}`. |
| `PUT /api/live/presence` | `{item \| null, position_ms, playing, following?}` -> `{now}`. Expires after 200 s without another. |
| `DELETE /api/live/presence` | Stop appearing at once -> `{now}`. |
| `POST /api/live/channels` | `{name, item?, position_ms?, playing?, locked?}` -> `201 {now, channel}`. The creator joins it. `locked` makes it a party. |
| `POST /api/live/channels/:id/join`, `/leave` | -> `{now, channel}`. A member is in one channel at a time. |
| `POST /api/live/channels/:id/playback` | `{playing, position_ms}` -> `{now, channel}`. |
| `POST /api/live/channels/:id/next` | `{from?, to?}` -> `{now, channel}`. `from` is ignored unless it is still the current entry, so every listener can report a song's end and it moves on once. `to` skips to a queue entry and drops the rows above it. |
| `POST /api/live/channels/:id/queue` | `{item, where?: "end" \| "next" \| "now"}` -> `201 {now, channel}`. Holds up to 200. |
| `DELETE /api/live/channels/:id/queue/:entry` | -> `{now, channel}`. |
| `POST /api/live/channels/:id/queue/:entry/move` | `{index}` -> `{now, channel}`. |

| `POST /api/live/channels/:id/end` | Host only. Ends it for everyone. |
| `POST /api/live/channels/:id/host` | `{user_id}`: hand over. Host only. |
| `PUT /api/live/channels/:id/controllers` | `{user_ids}`: who else may move the music. Host only. |
| `PUT` / `DELETE /api/live/channels/:id/skip-request` | Ask the host to skip this song, or take it back. Moving on clears them. |
| `PUT /api/live/channels/:id/background` | `{name, items (1 to 1000)}`: what plays, in a loop, when the queue runs out. Only someone at the controls may replace another's. |
| `DELETE /api/live/channels/:id/background` | Remove it. |
| `GET /api/badge/:rgb.gif` | No sign-in. The Discord card's badge, its rings lit in a colour of three hex digits. It names nothing else, and Discord fetches it, not the app. |
| `GET /api/open/party/:group/:party` | No sign-in. `302` to `siphon://party/...`, because a Discord button can only open https. It names no song or person, and the app lets in only those already in the group, after asking. |

In a party (`locked`), only the host and the controllers may seek, pause, skip or
use `where: "now"`; anyone else gets 403. Everyone may change the queue unless an
admin took that right. A guest's `next` works only with `from` as the current entry
once its time has really run out, so the music does not depend on the host being
there at that moment. A party whose host left passes to whoever has been there
longest. Channels without `locked` come from older clients and stay open to all
their members.

| `PUT /api/live/stats/month`, `/all` | `{summary}` -> `{now}`. A member's totals: this month's kept 40 days, all time's 400. |
| `GET /api/live/stats/month`, `/all` | `{now, stats: [{user_id, summary}]}`, for everyone who shared. |
| `DELETE /api/live/stats` | Take your totals back -> `{now}`. Also happens on leaving. |

`summary` is `{month, minutes, plays, songs, artists, genres, hours}`: the top
20 songs and artists and 10 genres at most, never individual plays.

`item` is one playable song, the share payload without album or playlist links.
Positions are extrapolated by clients from `at` against `now`. A channel is
`{id, name, host_id, created_at, members, current, position_ms, playing, at, queue,
locked?, controllers?, skip_requests?, background?}`. Its members are
those with live presence, so a channel ends about 200 seconds after its last
member closes the app. Up to 10 channels exist per group at once.

### Jev

The service asks TypeSafe AI's Jev decision model (pinned to `jev-1.14.0`)
typed questions and uses the probabilities quietly; no answer is ever shown
as a verdict, and a missing, slow (2.5 s) or failed answer reads as no
opinion. Without `TYPESAFE_API_KEY`, `AI_GATEWAY_API_KEY` or a Vercel OIDC
token, every question goes unanswered and the feed works as before.

- `GET /api/posts` adds `affinity` to each post: the reader's estimated
  probability, 0 to 1, of enjoying a friend's share, from their own 15 newest
  shares and hearts; `null` for their own shares or until they have two. Each
  share is scored once per reader and kept with the post.
- `POST /api/jev/next` ranks candidates as what should follow `recent`.
- `POST /api/jev/titles` splits upload titles into readings here and lets
  Jev pick one; only a pick of at least 0.6 is returned.

Every member may ask 30 Jev questions a minute and 3000 a day, on top of the
request limits above. Only titles, artist and uploader names and the
reader's own taste leave the service.

Payload: `{url, source, title, artists, artwork_url, duration_ms, note?, to?}`.
`to` is an optional list of up to 20 member ids. A share with recipients is
seen only by them and its author; everyone else gets 404 for it, including for
hearts and replies. Absent or empty means the whole group. Each share holds up
to 200 replies; a reply by a member who later leaves stays, as Former member.
`source` is `spotify`, `soundcloud` or `youtube`; `artists` is an array of names,
`artwork_url` may be null, and `duration_ms` is an integer (zero means unknown).
Post fields also include `id`, `author_id`, `author_name`, `created_at` (Unix
milliseconds), `like_count`, `liked` for the authenticated reader, `to`, and
`replies` (oldest first, each with its author's current name).

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

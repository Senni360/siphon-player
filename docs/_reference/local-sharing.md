---
title: Local Sharing and Parties
description: Personal-fork design and an opt-in hosted MP3 diagnostic. Not released party or local Cast support.
nav_order: 22
---

## Status and scope

This is the personal fork's implementation plan, inspected on 2026-10-30.
The maintainer authorized designing shared local recordings behind `senni.dev`
and the bounded diagnostic below. No infrastructure has been provisioned and
no recording uploaded to a cloud service. The diagnostic and locally tested
private-media gateway are independently reviewable parts of milestone 1.
The hosted and hardware acceptance gate remains open. Listening parties are
not yet supported.

The explicit fork exception is to CONTRIBUTING's Spotify-only scope, prohibition
on another backend or operated service, and AGENTS' hosted-backend boundary, for
user-selected local recordings and room coordination only. It does not permit
Spotify audio substitution, relaying, DRM circumvention, library uploads,
telemetry, new settings, deployments or spending. The local-library browser
decision remains separate. Future interface changes require concrete visual
scope approval and matching light/dark, narrow/normal evidence outside the repo.

## What exists

- `src/local_files.rs` indexes one chosen folder off the UI thread, matching
  tags and duration to `spotify:local:` identities. MP3, MP4, M4P and FLAC are
  scanned. The pinned librespot player has a separate startup index. WAV is
  neither indexed nor enabled in the current Symphonia feature set. There is
  no upload, hosted identity, arbitrary-file queue or party service.
- `HybridQueue` owns occurrence IDs, manual/context order, shuffle, repeat and
  paused persistence. `soundcloud::Source` chooses Spotify or SoundCloud by URI.
  `app::hybrid` projects optimistic state and rejects stale child events using
  occurrence and load generation. `backend::hybrid` stops the old engine before
  claiming the shared output. Mixed queues explicitly refuse remote transfer,
  including Spotify rows; pure Spotify playback keeps Connect.
- The SoundCloud engine already consumes resolved progressive MP3 and AAC/HLS
  URLs through bounded range/segment reads, seeks, cancellation and correlated
  URL refresh. It writes through the shared native output, EQ and visualizer
  tap. Its generic decoding can inform later hosted playback, but provider
  resolution and authorization must remain separate. No broad engine refactor
  is needed for this spike.
- `src/zeroconf.rs` discovers Cast devices; `src/cast.rs` authenticates them and launches
  Spotify's receiver, exchanges a Spotify receiver token, then hands control to
  Connect. It does not serve local bytes. The new `cast::media` diagnostic path
  launches Google's Default Media Receiver independently, with no provider
  credentials. Spotify activation is unchanged.
- `services/media-gateway` is a bounded, operator-only test service with private
  R2 reads, opaque temporary capabilities, revocation and retention cleanup.
  Its SQLite registry and local Workers runtime have been tested. It has no
  upload endpoint, participant authentication or production route. See the
  [service runbook](https://github.com/Senni360/siphon-player/tree/main/services/media-gateway)
  for exact limits, configuration and the separately required deployment review.

Inspected contracts and tests: how-it-connects, queue, what-spotify-allows and
soundcloud-engine; local-files matching; hybrid duplicate completions, stale
Spotify state, seek/pause intent and output ownership; SoundCloud range seeking,
expiry/refresh and cancellation; Cast authentication and fake-device activation.

## Existing infrastructure and proposed changes

Read-only inventory found `senni.dev` on Cloudflare nameservers, proxied DNS,
and HTTPS responses with Vercel headers. Authenticated Vercel inspection found
the existing `senni-dev` production project and apex/www domain association.
The private `Senni360/senni-dev` repository's Vercel configuration builds Vite
to `dist`; its inspected tree had no API, room, R2 or infrastructure configuration.
`media.senni.dev` and `party.senni.dev` resolve but returned Cloudflare 525.
DNS alone does not establish a usable service. Cloudflare account resources,
subscription, access controls and any unpublished infrastructure remain unknown.
The existing site and apex deployment need no change.

Approval proposal, before any provisioning:

| Resource | Proposed change and purpose |
| --- | --- |
| R2 Standard | Private `siphon-media-pilot` bucket, public access disabled, lifecycle for abandoned multipart uploads and hard expiry |
| `siphon-media` Worker on `media.senni.dev` | Validate expiring playback grants on every HEAD/range GET; stream authorized R2 ranges with HTTPS and no shared cache |
| `siphon-party` Worker on `party.senni.dev` | Authenticated room and upload APIs plus WSS upgrade; separate room/member identities from provider accounts |
| SQLite Durable Objects | One authoritative object per room, persisted revisions, commands and membership; hibernating WebSockets |
| Scheduled Worker cleanup | Reconcile expired grants/assets/uploads, tombstones and deletion retries; no retained filenames or library metadata |
| DNS/routes and secrets | Add only approved subdomain routes; scoped R2 signing credentials, rotating grant-signing keys and room-session authentication configuration |

These are proposed names, not existing resources. Stage M1 deployment to the
private bucket, media gateway and minimal grant/revocation registry for one
synthetic test clip. Enable upload admission in M2 and room WebSockets in M3.
Confirm account ownership, existing routes, storage region and actual billing
plan before applying a reviewed configuration. No DNS records are overwritten
on the strength of the public inventory alone.

Uploads go directly to the R2 S3 endpoint using short-lived authorization for a
specific owner, object, multipart upload and part number. R2 presigned S3 URLs
[cannot use custom domains](https://developers.cloudflare.com/r2/api/s3/presigned-urls/).
The user-facing media domain therefore uses an authenticated Worker for reads;
it does not make the bucket public. No large upload goes through Vercel or the
Worker. API requests reserve quota before issuing any parts. Completion verifies
size, digest, format and expected parts before making an asset ready.

[Durable Object WebSocket hibernation](https://developers.cloudflare.com/durable-objects/best-practices/websockets/)
fits one room authority, durable recovery and low idle cost. All desktops make
outbound WSS connections, avoiding host port forwarding and a TURN relay. Audio
uses separate HTTPS storage reads. Vercel now has
[WebSocket support in beta](https://vercel.com/kb/guide/do-vercel-serverless-functions-support-websocket-connections),
but bounded connection lifetimes and external multi-instance ordering add work.
Do not assume ordinary request handlers can be permanent in-memory room servers.

At the published pricing inspected on 2026-10-30,
[R2 Standard](https://developers.cloudflare.com/r2/pricing/) is $0.015/GB-month,
with 10 GB storage, 1 million Class A and 10 million Class B operations included;
additional operations are $4.50/$0.36 per million respectively and R2 egress is
free. [Workers Paid](https://developers.cloudflare.com/workers/platform/pricing/)
starts at $5/month with 10 million requests and 30 million CPU ms included.
[Durable Objects](https://developers.cloudflare.com/durable-objects/platform/pricing/)
include 1 million requests, 400,000 GB-s and 5 GB SQLite storage on Paid.

A small pilot capped at 10 decimal GB is approximately $5/month if the account's
included allocations are otherwise unused. At 100 GB steady retained storage,
that becomes approximately $6.35/month before any compute/operation overages.
These are estimates, not spending caps, and exclude tax, domain renewal and the
existing website bill. Four listeners at 320 kbps for two hours read about
1.15 GB; repeated seeks add requests. A PCM WAV can cost much more storage and
transfer than MP3. Explicit quota and budget enforcement must precede uploads.

## Media lifecycle

Proposed pilot limits, not enabled settings: MP3 first, 512 MiB/file,
2 decimal GB/member, 10 decimal GB total pilot storage, current plus at most
two explicitly queued upcoming recordings eligible for upload, two part uploads
concurrently and 8 MiB parts. Library scanning must never schedule uploads.
Initial auto-cleanup: 24 hours after last room use, seven days hard retention,
and abort unfinished multipart sessions after 24 hours. A sweeper retries
deletion, while a tombstone revokes access immediately. Lifecycle is a backstop,
not the sole deletion acknowledgement. Shared object references prevent deleting
another authorized queue entry's recording prematurely; hard retention wins.

Adding a local file to a party explicitly authorizes that recording. Present
bytes, state and cancellation beside that action. An upload remains unavailable
until verified completion; model `unavailable`, `uploading`, `ready`, `expired`
and `failed` separately. Cancellation invalidates the upload generation, aborts
multipart work and reconciles ambiguous completion; a late success cannot
resurrect a cancelled entry. Resume only the same owner/object/upload with the
same digest and size, after verifying the selected file has not changed.
Hash off the UI thread. Deduplicate within an owner/room authorization boundary
using verified content digests; knowing a hash must never reveal or grant access
to another owner's object. Use random opaque asset IDs in the room.

Keep local paths, tag blobs, artwork, unrelated metadata and provider credentials
out of room messages, object names and logs. Explicit display title and validated
duration/codec are sufficient. File tags remain present in the selected bytes;
the upload consent must disclose this. Metadata stripping or compressed listening
copies need a separate explicit design. Never silently transcode originals.

Issue playback grants near load, not on queue insertion. Proposed grants last
two hours and include asset generation, room/member scope, epoch and expiry;
refresh before the remaining lifetime falls below playback plus seeking margin.
The receiver uses a bearer URL, not cookies or provider authorization headers.
Authorize every range/HEAD, use `Cache-Control: private, no-store`, and support
206/Content-Range/Content-Length, 416, MIME and receiver CORS as validated in M1.
Revoking a member or deleting an asset rejects future reads, including old URLs.
Already buffered bytes cannot be recalled. Pause beyond expiry obtains a new
grant and reloads at the retained position; report that interruption. Refresh
must not reset the queue or accept a response for an obsolete load.

## Room contract

Build same-source mixed queues first: an entry identifies one local asset or
one provider track which every participant plays through that provider. Each
person supplies their own account and authorized playback. Spotify audio is
never uploaded, proxied or relayed. Spotify Jam is not this room protocol.

The room stores occurrence IDs, source/recording identity, readiness, ordered
revision and a timeline anchor `{epoch, occurrence, position, serverTime, playing}`.
Commands have actor, stable command ID, expected revision and intent. The single
authority authenticates membership before handling retries, deduplicates IDs in
a durable bounded window, and returns the original result. Old client epochs
must resnapshot, not replay after the dedup window. Removed members cannot retry
commands. Queue removal targets an occurrence, never every matching recording.

Default roles: owner controls transport, skips, removes any row and membership;
members add approved recordings and remove their own pending rows. Only an
explicit owner handover grants transport control. Owner loss keeps a 30-second
grace, then pauses with a durable revision; the owner can resume after reconnect,
or explicitly authorize a successor. No competing desktop elects itself owner.

For the closed pilot, propose owner-issued, single-use expiring invitations
exchanged for revocable device sessions stored in the native credential store.
Store only invitation/session verifiers server-side. Provider logins confer no
room membership. This authentication flow is future work and needs the native
credential round-trip checks before enabling it.

Apply local intent immediately as a pending overlay over the latest confirmed
revision. Rebase pending idempotent commands over acknowledged snapshots without
letting stale replies make rows or transport flicker away. Rejections remain
visible and actionable. Resume with last epoch/revision, receive contiguous deltas
or a full snapshot, discard stale/reordered messages and resend only pending IDs.
Persist timeline anchors and dedup state before acknowledgement so room restarts
do not repeat skips. Backoff reconnects with jitter and a bounded pending queue.

Estimate server-clock offset from several request/response midpoint samples,
prefer low-RTT samples and track uncertainty; run desktop position from monotonic
time between fresh anchors. Schedule starts at a future server time after a
bounded readiness barrier. A late or buffering participant joins at the current
anchor rather than holding everyone indefinitely. Freeze local estimates when
disconnected or after sleep until a fresh snapshot/clock sample arrives. Corrections
use a deadband and bounded seeks; no sample-accurate guarantee or unsupported
playback-rate control. An uploading head holds the room until ready or explicitly
skipped by the owner. An unavailable head pauses with its reason. Once a ready
track starts, an unavailable participant stays silent and shows the failure;
others continue on the same source. A ten-second initial readiness deadline
lets the owner start with ready participants or keep waiting. No participant
silently substitutes a provider or advances the shared queue locally.

Provisional acceptance targets: p95 inter-desktop audible offset at most 250 ms,
and Cast versus desktop at most one second after five seconds settling. Measure
over ten minutes with repeated starts/seeks, buffering and a reconnect using
synthetic acoustic markers captured on a common recorder. Also retain monotonic
command/status timings; reported player positions alone cannot measure output
latency. **No synchronization measurement has been made.**

## Chromecast and provider gates

Google lists [MP3 and WAV/LPCM](https://developers.google.com/cast/docs/media)
among supported formats, but device generation, sample format, receiver and
firmware still need real tests. Start with progressive 44.1 kHz stereo MP3.
WAV remains a separate decoder/receiver spike; its extension does not prove
support for every PCM layout. Temporary URLs must be fetchable by the receiver
itself with trusted HTTPS and range support. LAN discovery/control and receiver
internet reachability are different requirements; guest-network isolation matters.

Keep Spotify's receiver/token flow intact. The hosted path uses the Default
Media Receiver. Later, remember physical device preference separately from its
current receiver application. Source changes can launch another app and fail;
do not promise gapless app handoff. Keep the selected occurrence and position,
explain the failed handoff and offer local playback. This diagnostic does not
change the application's device picker or its local-only mixed queue restriction.

SoundCloud casting requires its own receiver/CDN access, authorization, refresh
and permitted-playback validation. YouTube is a separate feasibility and policy
gate; the existing download helper establishes no player support. Different
providers for the same recording is a later proposal: identifiers, alternate
versions, duration differences, availability, uncertain-match confirmation and
timeline mapping must all be validated before promising it.

## Milestones and acceptance

1. **Design and technical spike:** this plan and opt-in diagnostic. Approve exact
   hosting changes, use approved synthetic media, then validate desktop and real
   Cast playback, seeks, status progression, URL expiry/revocation, refreshed
   reload and deletion on two computers and one Cast. Record device/firmware,
   network and actual platforms. No support claim until this passes.
2. **Local sharing:** explicit selected-file upload with bounded lookahead,
   progress, resumable cancellation, authenticated playback, deletion/expiry.
   Test duplicate completion, file changes, reserved quota, unauthorized hashes,
   expiry, cancel/complete races, orphan abort and cleanup retries before enabling
   upload. Review the minimal UI evidence before merging visual changes.
3. **Local-file parties:** two remote desktops, ordered shared queue, roles,
   deterministic test clocks, command dedup, stale-state protection, late joins,
   buffering, reconnect and owner departure. Measure the desktop target above.
4. **Cast in parties:** desktop plus real Cast, measured offset, receiver app
   handoff/failure, seeking and expired-grant recovery. Local recovery remains usable.
5. **Mixed-provider parties:** validate one additional provider at a time with
   each participant's own account; test unavailable members and source changes.
6. **Cross-provider matching:** separate proposal after source-consistent rooms
   pass. No automatic uncertain substitutions.

## Running the bounded diagnostic

Use a private file outside the repository containing:

```json
{"url":"https://media.senni.dev/approved-test.mp3?grant=PRIVATE", "expires_at_unix":0, "duration_seconds":30}
```

Replace expiry with the actual server-issued Unix expiry. Do not put the URL in
shell arguments, screenshots or bug reports. The file is secret and must be
removed after testing. The probe does not read settings or provider grants,
upload anything or write audio to disk. It requires at least recording duration
plus ten minutes access remaining, HTTPS, no redirects, identity encoding,
`audio/mpeg`, exact range responses and a recording of at most 16 MiB.
The first read must supply a strong ETag. Later desktop/preflight range reads
send `If-Match` and reject a changed ETag even when the byte length is unchanged.
The Cast receiver fetches the URL itself, so the gateway must independently
enforce the asset's immutable stored object version on every request.
For a local HTTPS emulator only, `--ca-certificate /private/test-cert.pem` adds
one explicitly supplied certificate to the diagnostic's trust roots without
disabling TLS or changing system trust. It cannot be combined with `--cast`:
the receiver needs its own trusted public HTTPS endpoint.

```sh
cargo run --locked --no-default-features --example hosted_media_probe -- /private/test.json --approved-test-media
cargo run --locked --no-default-features --example hosted_media_probe -- /private/test.json --approved-test-media --desktop --seek-seconds 5
cargo run --locked --no-default-features --example hosted_media_probe -- /private/test.json --approved-test-media --cast 192.168.1.20 --seek-seconds 10
cargo run --locked --no-default-features --example hosted_media_probe -- /private/test.json --approved-test-media --expect-denied
```

For offline regression checks, without accounts or audio hardware:

```sh
cargo test --locked --no-default-features --lib cast::
cargo test --locked --no-default-features --example hosted_media_probe
# Requires ffmpeg; generates a synthetic MP3 in memory, never uploads it.
cargo test --locked --no-default-features --example hosted_media_probe -- --include-ignored
```

The first command checks the first and last byte ranges. Desktop downloads the
small test to bounded memory, seeks and plays at 10% gain through the existing
native sink. This is a diagnostic, not production streaming or room playback.
Cast authenticates the device, replaces its current receiver app, loads the
URL, polls position, seeks after two seconds and attempts STOP on completion or
failure. If interrupted or STOP fails, use the device's normal controls. A
fresh invocation with a refreshed manifest tests recovery at a known position.
After server expiry or revocation the last command requires a denied fresh read;
successful preflight alone does not validate revocation or actual playback.

The next bounded milestone is completing M1's approved hosted/hardware matrix,
then implementing upload admission and cancellation in M2. The M1 gateway needs
only an operator `ADMIN_SECRET` and service bindings. It stores capability
hashes, not signed grants, and requires no separate media signing key. Later
direct uploads need scoped R2 multipart signing credentials; rooms need their
own session authentication. These secrets stay server-side. Native clients
receive only scoped short-lived upload/playback grants. No real credentials
are required to run regression tests.

## Validation record (2026-10-30)

Windows code-level checks passed: six new Cast media tests, eight probe tests
(including the explicitly run ffmpeg synthetic decode/seek/EOF check), sixteen
existing Cast tests, ninety-three hybrid queue/backend tests and seventeen
SoundCloud engine tests. The live SoundCloud smoke test stayed ignored. These
140 passing tests are a focused subset, not the full application test suite.
`cargo clippy --locked --no-default-features --all-targets -- -D warnings`
also passed. The runnable diagnostic built and its `--help` command completed
successfully with `--no-default-features`.
The HTTP fixture uses loopback HTTP to test request/body behavior; it does not
establish hosted HTTPS or receiver interoperability.

Dependency-source, review-evidence and new-settings policy checks passed.
`cargo fmt --all --check` and `git diff --check` passed. The pre-existing module
ordering issue in `src/app.rs` was corrected in a separate formatting commit.

All six required default/all-feature Cargo Clippy, test, doctest and rustdoc
commands passed on Windows after configuring the already installed Visual
Studio, vcpkg and libclang build tools. The default library suite passed 1,743
tests and the all-feature library suite passed 1,785, with five ignored in each.
The no-default-feature suite also passed. Launcher checks reported three
failures and five errors from missing Unix/Ruby tools and Windows path handling.
Local Jekyll could not run without Bundler/Ruby. GitHub's Linux Docs job did
successfully build the site for `cf4d941`, then failed to upload its artifact
because the account's artifact storage quota was exhausted. No Linux or macOS
Rust build was tested locally. These remaining gates are not a fully green
contribution run.
The same GitHub checkpoint passed policy, quality and CI documentation jobs.
Its Linux test runner exhausted disk space; its macOS default suite reported
three existing `single_instance` handoff failures. These failures remain open
and are separate from the passing Windows result.
The follow-up gives each test a short process/counter socket directory to fit
macOS's path limit, preserving isolation and assertions. Its twelve focused
tests pass on Windows; the macOS rerun is still required.

The gateway passed 27 Node tests using SQLite and deterministic clocks. A
separate Wrangler 4.156.0 local Workers run used real local SQLite/R2 bindings,
HTTPS and a generated 12-second 44.1 kHz stereo MP3. Registration and grant
retries, full and range reads, HEAD, ETags, conditional reads, revocation and
expiry passed. A retention alarm deleted a second test object, confirmed by an
independent local R2 read. This validates the emulator, not a deployed
Cloudflare account, public DNS/TLS or receiver reachability.
The native probe passed first/final HTTPS range reads with an explicitly trusted
local test certificate, decoded the recording and completed five seconds through
the Windows default audio sink starting at position five seconds. After the
operator CLI deleted the asset, the native denial check passed with the same
manifest. Audible quality and the perceived seek point were not independently
confirmed. TLS verification remained enabled; the untrusted certificate was
rejected before adding explicit test trust.

A five-second IPv4 mDNS query on the default interface found no Cast
advertisements; this cannot rule out devices behind another interface or network
isolation. No real receiver, acoustic measurement, two-computer room or
cloud-hosted expiry/revocation/cleanup was validated. Generated test audio
stayed in disposable local storage; no infrastructure was changed. Measured
synchronization: **none**.

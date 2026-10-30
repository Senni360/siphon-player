# Private test-media gateway

This is the locally tested service portion of Siphon's first
[local-sharing milestone](../../docs/_reference/local-sharing.md). It serves
**already uploaded, explicitly approved synthetic MP3 test recordings** from a
private R2 bucket. It does not upload files, run rooms, accept participant
accounts or access provider credentials. No deployment or real receiver test
has been performed. The prepared configuration is not permission to deploy.

The module Worker forwards requests to one SQLite Durable Object named
`m1-pilot-v1`. That object holds authorization, immutable object identity and
cleanup state. Every playback request checks its capability in that registry,
including HEAD, later ranges and full GETs. Audio comes from R2 and is streamed
without buffering the file in the Worker. Nothing goes through the website or
a Vercel Function. This singleton is a bounded technical spike, not a room
service or a scalable general media-hosting design.

## Run locally

Node 24 or later is sufficient. There are no npm dependencies and no install
step. From this directory:

```sh
npm run check
npm test
node admin.mjs --help
```

The tests use actual SQLite through Node's built-in `node:sqlite`, a small
Durable Object storage adapter and fake R2 bindings. Time advances explicitly
in tests. No network, account, cloud service, audio device or provider is used.
Node currently labels its SQLite API experimental. These unit tests are not a
Workers emulator. A separate Wrangler 4.156.0 local runtime check on Windows
passed on 2026-10-30 with real local SQLite/R2 bindings and HTTPS: registration,
grant retries, full/range reads, conditional ETags, revocation, expiry and an
alarm that physically deleted a generated MP3 from local R2. Cloud routing,
account configuration and public receiver reachability still require an
approved deployment test.

The Rust diagnostic also passed HTTPS ranges against that emulator, decoded and
submitted a five-second segment starting at five seconds to the Windows default
audio sink, and rejected the same manifest after deletion through the admin CLI.
Use its `--ca-certificate` option for an explicitly trusted local test certificate;
TLS remains verified. A receiver cannot use this trust override. Audible quality
and seek accuracy were not independently measured.

## Bootstrap and deployment approval

Before enabling the service, separately approve and verify:

- Worker `siphon-media`, private R2 Standard bucket `siphon-media-pilot`, one
  SQLite Durable Object binding `MEDIA_REGISTRY`, and a 15-minute cleanup cron.
  Current plan, account ownership, region/jurisdiction and billing must be checked.
- `MEDIA_BUCKET` R2 binding and `MEDIA_ORIGIN=https://media.senni.dev`.
  `wrangler.toml` includes these proposed bindings and the SQLite migration.
  It has no routes and disables `workers_dev`, preview URLs and observability.
  No remote create/deploy command has been run.
- A private `ADMIN_SECRET`, exactly 32 random bytes represented as 64 lowercase
  hex characters, supplied as a Worker secret. No placeholder/default secret
  works. There is no public bootstrap, signup or anonymous grant issuer.
- Custom-domain routing and a trusted HTTPS certificate for `media.senni.dev`,
  after inspecting the existing DNS/routes. Leave the apex website untouched.
- A private bucket with public/R2.dev access disabled. Add a separately reviewed
  24-hour lifecycle backstop for the dedicated `spike/` prefix before uploading
  test media. Objects uploaded but never registered are outside the registry;
  delete them explicitly if registration fails, and use the lifecycle backstop.
- Request logging/data-access policy that does not retain full playback URLs
  or Authorization headers. The code never logs them, and Worker observability
  is disabled. Account-wide access logs, tracing or an external proxy may still
  capture them; deployment review must inspect those settings.

The M1 cost assumption remains the plan's approximately $5/month Workers Paid
minimum with storage/operations within otherwise unused included allowances.
Hard service quotas below limit admitted test media; they are not a billing
cap or a defense against unlimited anonymous request traffic. Review account
request limits and abuse controls before publishing a route.

The admin secret belongs only to the operator's private file and the Worker
secret store. It is never given to a receiver or participant. Rotating that
secret changes admin access; revoke playback grants separately if needed.
Do not restore a historical authorization database into a live route: disable
the route and invalidate its grants before a point-in-time restore, otherwise
past authorization could reappear. This pilot intentionally has no restore API.

## API and limits

All paths require the configured HTTPS origin. IDs are lowercase UUIDv4 values.
No endpoint accepts an arbitrary R2 key, filename, local path or audio body.
An asset's only possible storage key is `spike/<asset-id>.mp3`. Keys must never
be reused or overwritten. Admin calls use `Authorization: Bearer <ADMIN_SECRET>`
and, for PUT, `Content-Type: application/json`.

| Request | Behavior |
| --- | --- |
| `PUT /admin/assets/{id}` | Register an approved existing object after R2 HEAD verifies declared size and raw ETag, `audio/mpeg`, no content encoding and a stable R2 version |
| `PUT /admin/grants/{id}` | Register one capability hash for one asset and absolute expiry; returns no playback token |
| `DELETE /admin/grants/{id}` | Revoke the capability immediately; repeated deletion is safe |
| `DELETE /admin/assets/{id}` | Persist a tombstone before deleting R2 bytes; returns 204 when deleted or 202 while a retry is pending |
| `POST /admin/cleanup` | Explicit sweep; returns pending deletion count rather than claiming failed deletion completed |
| `GET /v1/media/{id}?grant={capability}` | Authorized full or single-range stream, pinned to registered size, raw ETag and R2 object version |
| `HEAD /v1/media/{id}?grant={capability}` | Same authorization and object identity checks, full content headers and no audio body |
| `OPTIONS /v1/media/{id}` | Anonymous CORS preflight only; reveals no object existence or media metadata |

An identical PUT can be retried. Different metadata under the same ID returns
409. A revoked grant cannot be reissued under its existing ID. Revoking an
unknown grant reserves that ID for two hours so a delayed issuance cannot
undo it. Deleting an unknown asset also tombstones it before an in-flight
registration can finish.

Hard M1 limits: 16 MiB/file, 16 not-yet-deleted assets, 128 MiB total registered
storage, 128 grant records, four KiB/admin body and a five-second body deadline.
Expired/revoked grant records count until cleanup removes them. Failed R2
deletions still count against storage quotas. At most 64 asset identities may
ever be registered or tombstoned in this pilot registry; permanent tombstones
prevent reuse without growing storage indefinitely. Reaching that lifetime
limit requires a reviewed new pilot registry, not silently forgetting old IDs.

Declared recordings must be 10 to 3600 seconds. Registration explicitly attests
`approvedSyntheticMedia: true`; its expiry must be within the next 24 hours.
**HEAD validates metadata, not MP3 contents.** Inspect the generated file locally
(for example with ffprobe) before upload. After registration, validate the actual
44.1 kHz stereo MP3 with the native diagnostic and on real receivers before
claiming compatibility. There is no transcoding, metadata stripping or decoding
inside this service.

Grants last at most two hours and never outlive asset retention. The client
generates an independent random 256-bit capability, and sends only its SHA-256
hash to the admin issuer. This hash is unrelated to the recording's file hash.
Only the receiver's later playback URL contains the capability. The registry
stores no bearer URL or capability plaintext. This simpler opaque-grant design
needs no additional signing key; room/member-bound signed grants remain later
work. Whoever receives the URL can use it until expiry/revocation. Do not share
an admin file or grant job with participants.

## HTTP contract

All responses use `Cache-Control: private, no-store` and `Referrer-Policy:
no-referrer`. Media responses allow CORS `*` without cookies or credentials and
expose Content-Type, Content-Length, Content-Range, Accept-Ranges and ETag.
No authenticated audio response is placed in a shared cache.

GET supports `bytes=start-end`, `bytes=start-` and `bytes=-suffix`. Ends beyond
EOF are clamped. Invalid/unsatisfiable ranges and multiple ranges return 416
with `Content-Range: bytes */<size>`. HEAD ignores Range as required by HTTP.
The strong quoted R2 `httpEtag` is returned unchanged. `If-Match` must match
strongly or returns 412 before reading bytes; `*` is supported. Quoted commas
inside tags are handled as part of the tag, not as list separators.

A matching strong `If-Range` preserves a GET range. A nonmatching/weak tag,
date or unrecognized value ignores Range and returns the complete pinned
recording with 200. GET uses R2 `onlyIf.etagMatches` with the raw registered
ETag, and checks size, version and metadata again before handing out the body.
A replacement or missing object fails closed. The service never silently
adopts a new recording under the old capability.

Every HEAD/GET revalidates expiry and revocation after R2 returns, so revocation
while a storage read is pending wins before response admission. A response
already streaming or audio already buffered by a receiver cannot be recalled.
The next request is denied. A refreshed grant uses a new ID and capability;
the native player must reload at its retained position. No queue state is reset
by this service.

## Explicit operator workflow

This workflow is for after deployment and test-upload approval. The utility
never uploads media or creates resources. Keep all JSON files outside the
repository in an owner-only directory. Unix files created by the utility use
mode 0600 and exclusive creation; on Windows the directory must have an
appropriate private ACL. Existing output files are never overwritten.

Private credentials file:

```json
{"origin":"https://media.senni.dev","secret":"REPLACE_WITH_PRIVATE_64_HEX_ADMIN_SECRET"}
```

After an approved synthetic MP3 has been uploaded separately to the UUID key,
prepare a registration job with its actual size and **raw**, unquoted R2 ETag:

```json
{
  "action": "register",
  "asset_id": "00000000-0000-4000-8000-000000000001",
  "metadata": {
    "size": 123456,
    "etag": "REPLACE_WITH_RAW_R2_ETAG",
    "duration_seconds": 30,
    "expires_at": 0,
    "approvedSyntheticMedia": true
  }
}
```

Replace `expires_at` with an absolute **Unix millisecond** time within 24 hours,
and use a fresh generated UUID rather than copying this illustrative identity.
No input pathname or title goes to the service. Then explicitly run:

```sh
node admin.mjs apply --credentials /private/admin.json --job /private/register.json
node admin.mjs prepare --asset YOUR_UUID --duration 30 --out /private/grant.private.json
node admin.mjs apply --credentials /private/admin.json --job /private/grant.private.json --out /private/playback.private.json
```

`prepare` is entirely offline. `apply` makes exactly one admin request, refuses
redirects and has a 15-second network deadline. The private grant job retains
its stable ID, capability and expiry, so retrying an uncertain result does not
create another grant. The playback manifest uses `expires_at_unix` in **seconds**
for `hosted_media_probe`, unlike the service's millisecond API. It contains only
URL, expiry and duration. Neither utility output nor errors print its URL.
If writing the playback manifest fails, keep the grant job and retry to a new
private output path. Grants too close to expiry for recording plus ten minutes
are refused by the utility.

Use the native `hosted_media_probe` with that manifest to test desktop/Cast
seeking and strong ETag consistency. For revocation, use a private job
`{"action":"revoke","grant_id":"UUID_FROM_PRIVATE_GRANT_JOB"}` and `apply`.
For deletion use `{"action":"delete","asset_id":"YOUR_UUID"}`.
`{"action":"cleanup"}` asks for an explicit sweep. Never put a capability or
admin secret on the command line. Delete both private grant job and playback
manifest after the test, after revoking/deleting the recording as appropriate.

## Cleanup and validation limits

Asset expiry is absolute; playback does not extend it. Alarms perform expiry
cleanup without requiring another participant request. A failed R2 deletion
keeps its tombstone, records a retry and backs off from one minute to a maximum
15 minutes. Retries persist across object recreation. A 15-minute scheduled
Worker independently sweeps the known singleton, because the platform's alarm
failure retries are finite. A deletion acknowledged by R2 is idempotent; repeating
an alarm or DELETE does not restore access. Failed cleanup can retain physical
bytes until storage recovers, but expired/tombstoned access remains denied.

The tests cover auth isolation, strict ranges/HEAD/conditional ETags, expiry,
revocation and deletion races, duplicate commands, caps, restart persistence,
cleanup retries, CORS and error privacy. No audio, secret values or generated
test artifacts are committed. Unit tests represent media with fake bytes; the
separate local runtime check used a generated 12-second 44.1 kHz stereo MP3.
Real receiver format compatibility, byte-range performance, public HTTPS
reachability, Cast URL expiry, two-computer playback and acoustic synchronization
remain unverified.

Before claiming M1 complete, run the documented hosted desktop/Cast matrix,
verify revoked/expired links deny new reads, allow the retention alarm to fire,
confirm the object is actually gone, and test cleanup recovery after a simulated
storage failure. Repeat the local runtime checks against the approved cloud
deployment; a passing emulator does not establish deployed behavior.

Implementation references: [R2 Worker API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/),
[SQLite storage](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/),
[Durable Object alarms](https://developers.cloudflare.com/durable-objects/api/alarms/).

# Authoritative room queue core

This is an isolated, unreleased part of Siphon's
[local-sharing plan](../../docs/_reference/local-sharing.md). It implements a
durable queue authority and the queue-window check used by the isolated upload
ledger. It has **no server, WebSocket endpoint, participant authentication,
playback timeline, UI, provider integration or deployment configuration**.
No new service, secret, account, setting or spending change is needed.

Run with Node 24 or later, without installing dependencies:

```sh
cd services/party-room
npm run check
npm test
```

The tests use real SQLite through Node's built-in `node:sqlite`, fake membership
and media authorities, and deterministic clocks. They include the actual upload
ledger from `services/media-gateway`. No audio, network, library scan or provider
account is used. Node currently marks its SQLite API experimental.

An optional test uses a separately installed Miniflare tool, with no npm
dependency added to this service:

```sh
npm run test:runtime -- /absolute/path/to/node_modules/miniflare
```

The checked version was Miniflare `5.20261023.3-beta` from Wrangler `4.156.0`.
The runner creates a disposable local SQLite Durable Object with fixed fake
actors, disables telemetry/`cf` lookup/outbound requests, binds to loopback,
and disposes the runtime after testing. It reads no deployment config or secrets.
The fixture must never be deployed as an authentication or room endpoint.

## Commands and permissions

`RoomQueue(storage, options)` takes a SQLite Durable Object-shaped storage
adapter and trusted `roomId`, `ownerId`, `epoch`, `authorize`, `resolveRecording`
and `now` arguments. One storage authority owns one room. Reopening it must use
the same identity; constructing another owner or epoch cannot overwrite it.
The callbacks are synchronous and must not perform blocking network work.
Membership requires literal `true`; a promise, exception or missing grant fails
closed. A real session authenticator must supply the actor argument. Actor IDs
never come from command JSON.

Commands have exactly this shape:

```json
{
  "epoch": "00000000-0000-4000-8000-000000000005",
  "command_id": 1,
  "expected_revision": 0,
  "action": {
    "type": "add",
    "occurrence_id": "00000000-0000-4000-8000-000000000101",
    "recording_id": "00000000-0000-4000-8000-000000000201",
    "explicit_selection": true
  }
}
```

The IDs above are illustrative, not usable credentials. `command_id` is a
positive, consecutive integer scoped to `(room epoch, authenticated actor)`.
An actor starts at one and retains the same ID and payload when retrying.

| Action | Permission and effect |
| --- | --- |
| `add` | A member explicitly selects a local recording owned by that member. Append one new occurrence. Repeated recordings remain separate occurrences. |
| `remove` with `occurrence_id` | Members remove their own pending rows; the owner can remove any pending row. Current rows require owner advancement. |
| `advance` | Only the owner selects the next pending occurrence, or clears the current occurrence when the queue is empty. |

An uploading or unavailable occurrence can become current, but this does not
start audio. The future timeline must hold it until ready or explicitly skipped.
There is no automatic completion/advance, shuffle, repeat, context queue or
owner election. The existing desktop queue is untouched.

The media adapter must authorize the recording for that actor and return only
`{ owner_id, revision, state, duration_ms }`. Recording IDs are opaque UUIDs;
knowing an ID is insufficient to add someone else's recording. No filenames,
paths, titles, tags, file hashes, URLs or provider credentials enter this core.
The initial source is exclusively `shared_local`. Spotify, SoundCloud and
cross-provider matching are not implemented or promised here.

## Revisions, retries and persistence

Every accepted queue mutation advances the room revision once. Queue changes,
occurrence tombstones, the actor's command cursor and the command receipt commit
in one transaction. `storage.sync()` must complete before acknowledgement.
An uncertain acknowledgement can be retried with the original ID. It cannot
repeat an accepted skip or addition after ledger reconstruction.

Membership is rechecked before returning a stored receipt, including retries.
The same command ID with different content is a conflict. A stale expected
revision becomes a durable rejected receipt, consumes that ID and changes no
queue state. A later retry returns that original rejection. The client must
resnapshot and explicitly issue a new command if it wants to retry the intent.
Gaps and stale epochs are rejected before any action.

Each actor retains its latest 64 receipts. Older receipts are evicted, but a
durable high-water mark remains: an old command is rejected with `receipt_expired`,
never executed again. The snapshot returns only that actor's next command ID and
receipt floor. A client must not silently assign a fresh ID to an uncertain old
command; that could duplicate the user's action. It must reconcile the snapshot
and keep the unresolved intent visible for an explicit decision.

Acknowledgements contain `{ epoch, command_id, revision, accepted, error }`.
They are receipts, not snapshots. An old duplicate acknowledgement may correctly
carry an older revision than the current room. Future clients must ignore old
snapshots, retain pending optimistic intent, and retire an accepted overlay only
after seeing a snapshot at or beyond its acknowledged revision. A rejected intent
must remain visible and actionable. This increment does not wire or validate a
native client overlay, so it makes no claim of completed optimistic party UI.

## Readiness and upload admission

Readiness has three distinct states: `unavailable`, `uploading`, and `ready`.
Only a trusted media authority calls `publishMedia(recordingId, metadata)`.
Participant commands cannot mark a file ready. Media revisions are monotonic;
older status is ignored, conflicting equal revisions are rejected, and identity
ownership/duration cannot change under an existing recording ID. Repeated queue
occurrences reflect the same media status. A stale resolver cannot downgrade it.
The actual media verifier and access-grant service are still separate work.

`allowsUpload(actor, roomId, occurrenceId, operation)` is a synchronous authority
callback compatible with `UploadLedger`. New admission and part/completion work
require all of:

- Current membership in this exact room.
- The occurrence and recording belong to that actor.
- The recording is still uploading.
- The occurrence is current or one of the next two rows. Before any current
  occurrence is selected, the first three pending rows form that window.

Removed or consumed rows keep ownership tombstones, allowing their owner to read
upload progress and cancel. These operations remain available after logical room
expiry while membership is still valid. Revoked membership grants nothing.
The adapter checks permission and position; it does not verify file bytes or
grant playback access. The future coordinator must also cancel work leaving the
window, revoke outstanding capabilities where possible, unify storage quotas,
and associate verified media with the intended recording. Existing issued part
URLs are not revoked by changing the queue alone.

## Bounds and remaining gates

Pilot constants: 32 pending rows plus one current row, 256 lifetime occurrence
IDs, eight lifetime actors with command cursors, 64 receipts per actor, and a
24-hour room lifetime. Occurrence tombstones are never reused within the epoch.
Hitting a lifetime bound requires a new room epoch through a future lifecycle
API. Reopening the existing store does not reset limits. Expiry is a logical
admission/read boundary, not automatic physical deletion of SQLite data.
Retention cleanup, session revocation, quotas across rooms, request byte limits
and rate limits must be implemented before deployment.

This core has run in the local Durable Object emulator, but not a deployed
service. Production runtime wiring, authentication, WebSocket recovery, client overlays, authoritative timeline,
clock-offset estimation, buffering, owner departure/handover and playback
integration remain unimplemented. No two-computer party or Chromecast test has
run, and no synchronization behavior has been measured. The hosted/hardware
milestone-1 gate remains open. This module does not enable milestone 3 in the app.

Validation on 2026-10-30: seventeen focused Windows tests passed, covering queue
ownership, duplicate occurrences and commands, stale epochs/revisions, bounded
receipt eviction, media status, quota bounds, concurrent commands, persistence
failures, expiry and integration with upload eligibility/cancellation. Four
local Workers runtime cases also passed: duplicate receipts after core
reconstruction on the same SQLite store, ownership/stale-state rejection,
upload-window admission/cancellation, and stale media revisions. No process
crash, durable-object eviction, remote transport or playback acceptance was
tested. Full required Windows Rust checks and the 60 media-service tests also
passed. Launcher/Jekyll prerequisites and GitHub billing restrictions remain
the documented contribution-check limitations.

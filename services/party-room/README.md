# Authoritative room queue core

This is an isolated, unreleased part of Siphon's
[local-sharing plan](../../docs/_reference/local-sharing.md). It implements a
durable queue/timeline authority, an in-memory client recovery model and the queue-window
check used by the isolated upload ledger. It has **no server, WebSocket endpoint, participant authentication,
UI, audio playback, provider integration or deployment configuration**.
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
The optional trusted `canStart` callback defaults to refusing playback starts.
The [timeline contract](TIMELINE.md) defines its readiness gate and limitations.
The [clock contract](CLOCK.md) documents authenticated timestamp sampling and
bounded in-memory estimation, without adding an endpoint or a native adapter.
The [presence contract](PRESENCE.md) adds a durable owner-departure grace period;
new starts also require trusted connected-owner presence.

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

An uploading or unavailable occurrence can become current, but its timeline stays
paused until ready and explicitly started or skipped by the owner.
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
after seeing a snapshot at or beyond its acknowledged revision with a cursor
proving the command was consumed. The headless model below tests these rules.
It does not wire or validate a native overlay or complete the optimistic party UI.

## Headless client recovery

`RoomClient({ roomId, actorId })` in `src/client.mjs` models one authenticated
actor's queue intent. It is synchronous and in memory only. It neither connects
to a service nor drives playback. Its inputs must come from a future trusted
transport adapter; membership and upload authority remain server decisions.

| Method | Contract |
| --- | --- |
| `beginConnection()` | Returns a new generation. Pass it with every callback from that connection. Request a fresh snapshot before sending commands. |
| `disconnect(generation)` | Invalidates callbacks from the old connection and retains pending actions. |
| `receiveSnapshot(generation, snapshot)` | Accepts bounded, ordered snapshots from the same room and epoch. Rejects conflicting identity or media state. |
| `enqueue(action, preview?)` | Immediately overlays add, remove or advance. Add requires an exact public row preview and explicit selection. Returns a local intent ID. |
| `outbound()` | Returns one command to send, or `null`. Repeated calls return an unchanged copy until its outcome is resolved. |
| `receiveReceipt(generation, receipt)` | Records the matching outcome. Accepted intent retires only after the confirming snapshot also arrives, in either order. |
| `receiveFailure(generation, commandId, code)` | Consumes a normalized protocol failure. Temporary failures require a fresh snapshot; expired receipts and command conflicts stay uncertain. |
| `view()` | Returns a copy of the projected queue and each intent's status/error. This is display state, never playback or upload authorization. |

Only one command is outstanding at a time. Up to 16 pending intents remain
visible even offline, including rejected or uncertain intent. Later commands
wait for the first intent's outcome. The authoritative snapshot still has at
most 32 pending rows; concurrent remote additions can temporarily leave the
display projection with up to 48 pending rows while local intent is unresolved.
Nothing is silently truncated to fit the server bound.

An advance captures the current occurrence and intended next occurrence when
queued. If they change before dispatch, the intent enters `review` instead of
skipping a different recording. Multiple queued advances do not consume rows
twice when a confirming snapshot arrives before its receipt. Snapshot revisions,
the actor's command cursor and receipt floor cannot regress. Known occurrence
identity and recording ownership/duration cannot change, and repeated recording
rows must agree on media status and revision. Older connection callbacks are
ignored. An epoch change quarantines pending commands instead of replaying them.

Resolution requires an explicit future UI action:

- `discard(intentId)` removes only unsent, review or definitively rejected intent.
  It cannot cancel a sent command or erase an uncertain outcome.
- `retryRejected(intentId)` requires a fresh snapshot proving the old command was
  consumed. It queues that intent for a new command ID, with the same target checks.
- `acceptUncertainOutcome(intentId)` accepts the current server state only after
  its cursor proves the uncertain ID was consumed. It never resends the action
  under a fresh ID. It is not wire-command cancellation.

An expired room, changed epoch or exhausted lifetime actor capacity blocks
dispatch and requires a deliberate new-room/rejoin decision. The future adapter
must authenticate the actor, enforce message byte
limits, correlate receipts to that connection's requests (never broadcast another
device's receipts), schedule bounded reconnects and snapshot requests, and
durably journal pending intent and the exact packet before sending. This model
does **not** survive process exit, implement crash recovery, persist files, or
provide a socket retry loop. A native implementation must preserve these tested
rules and the project's atomic background-write contract. No settings or UI
changes are included.

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
service. Production runtime wiring, authentication, WebSocket recovery, native
client overlays, readiness coordination, native clock sampling, buffering,
session presence detection, alarm wiring, owner handover and playback
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

The in-memory client adds twenty-one deterministic Windows tests against the real
SQLite queue core. They cover snapshot/receipt ordering, repeated advances,
lost acknowledgements, old connections, command cursor changes without room
revision changes, uncertain receipt eviction and competing device commands,
explicit rejection retry, identity/media conflicts, exhausted actor capacity, bounded private-data-free
messages and unchanged retry packets. These are protocol model tests, not a
two-computer connection, native UI, playback or process-restart test.

The durable timeline adds twenty-three deterministic tests, for 61 room tests in
total. Seven local Durable Object cases cover the original queue checks plus
gated starts, persisted timeline reconstruction and media-loss pause. See
[TIMELINE.md](TIMELINE.md) for the command schema, validation and remaining gates.

The clock model adds 23 deterministic tests, bringing the room suite to 84. Eight
local Durable Object cases now include timestamp sampling that preserves queue
revisions and command cursors. The estimator is not connected to native playback;
its modeled uncertainty does not establish audible synchronization.

Owner-presence recovery adds twenty deterministic tests, for 104 room tests in
total, and a ninth local Durable Object case. It persists a 30-second loss
deadline, pauses once at that deadline and never auto-resumes after a late return.
The actual session authority, alarm and native pause integrations remain absent.

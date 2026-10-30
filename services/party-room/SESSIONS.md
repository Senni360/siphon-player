# Session leases and active-output fencing

`SessionRoom` composes the existing queue, timeline, presence and readiness
authority with bounded connection leases. It prevents a superseded output from
contributing readiness and derives owner presence from all live owner sessions.
It has no socket endpoint, authenticator, alarm scheduler, native integration or
audio control. It changes no provider behavior, infrastructure, credentials,
settings, upload quotas or media retention.

Run `npm run check` and `npm test` with Node 24 in this directory. The optional
local runtime command is in [README.md](README.md). No dependency installation is
required for the deterministic tests.

## Trust and composition

Construct `SessionRoom(storage, options)` from `src/sessions.mjs` with the same
identity, membership, verified-media and clock adapters as `RoomQueue`, plus:

`authorizeSession(actor, sessionId, roomId, operation) -> true | false`.

This synchronous adapter defaults to denying every session event. A promise,
exception or value other than literal `true` denies it. Room membership is also
required. The adapter must validate a trusted connection lifecycle or output
event. Session IDs are correlation fences, **never credentials**. A callback
that merely recognizes a participant-supplied ID is insufficient authentication.
The test fixtures use fixed fake proofs and must never be deployed.

The host must authenticate every incoming operation, derive the actor and
session context from that authenticated connection, and reject participant JSON
claiming authority over either. This also applies to inherited queue commands,
snapshots and clock probes: the new coordinator is not an HTTP security wrapper.
Network authentication happens outside the synchronous transaction. A trusted
revocation must close the affected sessions or revoke room membership promptly;
changing the proof callback alone does not immediately erase a cached ready vote.
Failed heartbeats eventually expire the lease, but are not immediate revocation.

Use only `SessionRoom` for a store once it has acquired session state. The raw
`RoomQueue` constructor now refuses a store with the session marker table, even
if that table is empty. A raw instance retained across upgrade also refuses its
transactional operations with `session_authority_required`; its upload callback
returns false. Replace every authority reference, including upload callbacks,
when upgrading. Existing raw stores remain supported until their first upgrade;
there is no downgrade API. SessionRoom reconstruction remains supported.

The internal subclass marker is a composition guard, not authentication or a
sandbox against privileged code with database access. It is not a serializable
participant option. The base class remains the isolated trusted-input protocol
core. Internal transition helpers are not transport endpoints.
SessionRoom refuses external `publishOwnerPresence` calls
with `presence_managed`, and replaces the readiness roster callback with the
current live-session roster, deduplicated by actor.

## Bounded lifecycle

All lifecycle operations take a trusted actor and exactly the fields shown:

| Method | Event | Effect |
| --- | --- | --- |
| `openSession` | `{epoch, session_id}` | Creates one new bounded lease. Repeating an ID returns its existing state without extending or reopening it. |
| `heartbeatSession` | `{epoch, session_id, revision}` | A strictly newer heartbeat revision extends a live lease, capped by its hard expiry. Old or duplicate reports never extend it. |
| `closeSession` | `{epoch, session_id}` | Closes exactly that actor's session, idempotently. An old close cannot affect its replacement. |
| `sessionStatus` | `{epoch, session_id}` | Returns only that actor's private session status and output cursor. |

The fixed spike bounds are two live sessions per actor, eight lifetime actors,
64 lifetime session IDs, a 15-second heartbeat lease and a 15-minute hard session
lifetime, capped at room expiry. The room's existing command/occurrence bounds
remain separate and unchanged. Closed, expired and revoked IDs remain tombstones
until room retention cleanup, which is not implemented. Hitting a lifetime bound
requires a future explicit room lifecycle flow; reconstruction never resets it.
These constants are not settings or measured mobile/Cast/network limits.

At the exact lease deadline, a session is inactive. A late heartbeat or duplicate
open cannot resurrect it. A replacement needs a new ID and explicit output
selection. There is no seamless session rotation or automatic late join in this
spike. A future adapter must handle re-preparation and user intent before these
bounds can support real listening sessions.

Lifecycle results contain `{applied, session, revision, next_alarm_at}`. Private
`session` details include state, lease/hard expiry, heartbeat cursor and the
actor's output cursor. They must not be broadcast to the room. Public snapshots
and client journals contain no session IDs, output revisions or heartbeat data.

## One selected output per actor

The trusted output adapter calls:

`publishOutput(actor, {epoch, session_id, revision})`.

`session_id` is an actor-owned live session, or `null` to clear the output. The
positive output revision belongs to the actor within the room epoch and never
resets for a new connection. An equal revision with different content is a
conflict; older events are ignored. A higher revision for the same session also
fences a replaced decoder or receiver. The adapter must allocate and retain that
sequence through recovery. This module does not identify a physical receiver or
validate Chromecast handoff.

Opening a connection never selects it automatically. Losing the selected
connection never selects a spare connection. An actor can remain present while
having no usable output. During preparation, that actor has `unavailable`
readiness and cannot vote ready until a live output is explicitly selected.

SessionRoom requires a separate trusted context argument for readiness:

```js
room.publishReadiness(actor,
  { epoch, generation, revision, status },
  { session_id, output_revision });
```

The final argument comes from the output adapter, not report JSON. Reports must
match both the captured load and the current selected session/output revision.
Replacing, clearing, closing, expiring or revoking that output removes its ready
vote in the same transaction. The actor's report high-water mark remains intact;
a new output must use the next report revision, not replay a previous `ready`.
Heartbeats neither refresh votes nor silently extend their original deadline.
A ready vote expires at the earliest of freshness, attempt expiry and the lease
deadline observed when reporting. It must still be valid after the start lead.

Output changes after a start remove that actor's startup admission without
pausing everyone else. They cannot join the already-started load by reporting
ready again. Native outputs must honor this invalidation and remain silent until
a new validated preparation or future late-join flow. The server cannot stop a
receiver by changing a SQLite row. Reports remain participant claims; the
independent default-denied `canStart` gate still validates media/access.

## Owner presence, durability and recovery

Any live owner session keeps aggregate presence connected, even when its output
is unavailable. Losing the last one starts the existing 30-second grace. A known
close uses its event time. A missed heartbeat uses the last owner's actual lease
expiry, so delayed recovery does not grant a fresh grace period. A late tick
freezes the authoritative position at that original expiry plus grace. A return
before the deadline preserves an existing running anchor; a return at or after
it leaves playback paused until explicit readiness-approved resume.

Session maintenance runs inside each successful room transaction. Call
`tickSessions()` for recovery without participant traffic; it returns
`{changed, revision, next_alarm_at}`. The host must schedule the earliest result
of `nextSessionAlarm()` after **every** operation, including queue/readiness
commands whose existing return shape contains only their own deadline. It must
also broadcast public state changes caused by reads or clock probes. The method
combines lease, presence, readiness and room-expiry deadlines. Logical room expiry
still refuses operations and does not delete stored data.

Session/output rows, readiness invalidation, presence transitions and resulting
timeline changes commit together and await storage durability. Public effects
advance the existing room revision; heartbeats do not consume participant
command IDs or unnecessarily invalidate queue commands. A lost storage reply is
uncertain: retry the original event, which retains its original lease/cursor.
A failed transaction rolls back all of its effects. Multiple authority instances
must not operate concurrently on one store.

On first upgrade of a legacy store, session history is unknown. Existing playing
state is paused at its projected position, readiness is invalidated and presence
becomes unknown. Existing command receipts and cursors survive. Reopening a
session-aware store preserves leases and output fences. Neither migration nor
recovery grants a session or automatically starts playback.

## Validation and remaining gates

Thirty-three deterministic Windows tests cover default-denied authorization,
ownership, bounds, stale/equal revisions, lost replies, rollback, reconstruction,
legacy upgrade, aggregate owner departure, exact lease/grace boundaries, output
replacement, old readiness callbacks, journal privacy and pending optimistic
transport intent, raw-authority refusal after upgrade and upload-callback
rewiring. The full room suite passes 243 tests. Ten local SQLite session cases
bring the runtime total to 25, including unchanged leases after reconstruction,
a late tick pausing at the original deadline and refusal of raw authority reuse. The runner
disposes its runtime. Core reconstruction is not cloud eviction or process-crash
validation, and these tests use deterministic clocks and fake connection proofs.

Production authentication/revocation, connection heartbeat delivery, alarms,
rate limits, cross-room quotas, retention, native journal integration, decoder
readiness, output control and physical-device handoff remain absent. No hosted
party, second computer, Chromecast or audible synchronization was tested. M1
cloud/hardware acceptance remains open. The
[local reconnect harness](RECONNECT.md) now exercises the session-bound client,
journal, clock and authority together with synthetic faults. Production
transport, authentication and playback integration remain absent.

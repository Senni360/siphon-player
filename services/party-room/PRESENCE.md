# Owner-departure grace and recovery

The room authority persists aggregate owner presence and a 30-second departure
grace period. This is an isolated backend contract. It does not implement session
authentication, detect sockets, schedule a real alarm or control an audio device.
No new endpoint, infrastructure, secret, setting or interface is introduced.
Run `npm run check` and `npm test` here with Node 24.

## Trusted presence authority

The base `RoomQueue` accepts events only from a trusted session authority:

`publishOwnerPresence({epoch, revision, connected})`.

The optional [SessionRoom coordinator](SESSIONS.md) now derives these transitions
atomically from its bounded connection leases and refuses external presence
events. Its default-denied connection-proof adapter still needs a real
authenticator and transport. The following base contract remains unchanged.

The event describes whether **any authenticated owner session** is connected.
A single socket closing is not proof that the owner has left when another owner
session remains. Participant commands cannot publish presence or elect an owner.
The owner identity remains immutable; explicit handover is still future work.

The event revision is a positive monotonic integer scoped to this room epoch.
Its high-water mark persists across reconstruction. Older events are ignored,
equal events are idempotent, and an equal revision with different content is a
conflict. A newer report with the same connected state updates only this mark;
it cannot restart a grace period. The adapter must preserve this event sequence
across recovery, rather than resetting it for each socket.

The result is `{applied, changed, revision, next_alarm_at}`. `applied` means the
event advanced the presence-event sequence. `changed` means public room state
changed, possibly because an overdue departure was recovered first. `revision`
is the shared room revision. Presence events do not consume participant command
IDs. Internal sequence values and session identities are not exposed to clients.

## State transitions

Snapshots include exactly `{state, pause_at, paused}` under `owner_presence`:

| State | Behavior |
| --- | --- |
| `unknown` | No trusted presence history. New Play commands are refused. |
| `connected` | Play still requires ready media and the separate readiness gate. |
| `disconnected`, before `pause_at` | Existing playback may continue. New Play commands are refused. |
| `disconnected`, `paused: true` | The deadline has been durably applied. Playback stays paused. |

The first accepted loss transition sets `pause_at` to 30 seconds after the room
records that transition, capped at room expiry. Detection or delivery latency
before the room learns of the loss is not covered by this bound. Repeated loss
reports, retries and reconstruction cannot extend the original deadline.

A return **before** the deadline cancels it and preserves an existing running
anchor. A return **at or after** the deadline first commits the pause, then
records the return. It never resumes playback automatically. The owner must
explicitly Play again through the readiness gate. Old disconnect reports cannot
undo a newer return.

The timeout freezes position at the original deadline, even if maintenance runs
late. Position clamps at EOF and never advances the queue. Already paused,
uploading, unavailable and empty selections remain valid. An earlier room expiry
remains the final read/admission boundary.

## Persistence and maintenance

One bounded `room_owner_presence` row stores the event sequence, connected state,
deadline and applied flag. Presence transitions and timeout effects share the
queue's transaction and ordered revision. The timeout's flag, paused anchor and
revision commit together and wait for durable acknowledgement. Lost replies do
not repeat the effect. A reconnect at an overdue deadline can advance two room
revisions in one transaction: the timeout followed by the return.

The future owner of this Durable Object must schedule `next_alarm_at` and call
`tickOwnerPresence()`, including with no participant traffic. An overdue alarm
must run immediately. The method returns `{changed, revision, next_alarm_at}`;
early and duplicate calls are harmless. It has no native timer or alarm wiring.

Successful room operations also recover overdue presence in their transaction.
This includes snapshots, clock samples, commands and trusted media updates. A
command based on the pre-timeout revision is therefore stale. A future transport
must broadcast the committed room state after maintenance, even when the
triggering operation was a read or clock probe. It must coordinate this alarm
with room expiry and eventual retention cleanup.

Legacy stores without presence history initialize as unknown. A legacy playing
anchor is paused at its current projected position and gets a new room revision;
old command receipts remain intact. The clock floor advances with that migration
so a later clock rollback cannot rewind the preserved position.

## Client and remaining gates

The headless queue client validates presence in snapshots and exposes a copy as
`confirmed_owner_presence`. It rejects conflicting state at an equal room
revision and keeps pending queue intent visible. It does not schedule a local
pause or change the native interface.

A future native adapter must honor an announced deadline using bounded clock
estimates, handle connection loss and retain optimistic intent. Server maintenance
alone cannot ensure receivers stop on time if the network or server is unavailable.
Session aggregation is now modeled by SessionRoom. Real heartbeat/disconnect detection, authenticated transport,
alarm wiring, native output control and owner handover remain unimplemented.
Those integrations and real hardware tests are required before party support.

## Validation

Twenty deterministic Windows tests cover exact deadlines, late maintenance,
reconstruction, old/conflicting events, duplicate acknowledgements, atomic
storage failures, early/late return, explicit readiness-approved resume, clock
rollback, expiry, legacy-store initialization and pending client intent. The
complete room suite passes 104 tests. Nine local Durable Object cases pass,
including persisted departure recovery and a return that stays paused. The
runtime is disposed afterward. No real session loss, two-computer playback,
Chromecast or acoustic synchronization has been measured.

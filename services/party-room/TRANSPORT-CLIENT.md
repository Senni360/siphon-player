# Pending transport intent

The headless `RoomClient` now handles owner transport intent alongside queue
intent. This is an in-memory protocol model, not a native interface, audio
controller, socket or persistent journal. It adds no infrastructure, credentials,
settings, provider behavior, media storage or transfer.

Run `npm run check` and `npm test` in this directory with Node 24. The optional
local Durable Object runner is documented in [README.md](README.md).

## Enqueue and display

`enqueueTransport(action, {position_ms})` accepts the existing `prepare`, `play`,
`start_ready`, `pause` and `seek` actions from the [timeline](TIMELINE.md) and
[readiness](READINESS.md) contracts. Only the room owner may enqueue them.
They must target the occurrence currently shown by the queue projection,
including a pending advance. The preview contains exactly one bounded integer
position, from zero through the recording duration. For Seek it must match the
action's position. No filename, URL, credential or extra metadata is accepted.

The preview is the position the caller is displaying when the control is used.
It is never sent as an authority claim for Pause, Prepare or Play. The server
continues to project its own timeline when executing those commands. No clock
estimate or local device state is inferred by this model.

`view().pending_transport` immediately exposes the latest pending transport:

`{intent_id, occurrence_id, action, position_ms, playing, applies_to_current, state, error}`.

Play and `start_ready` show desired playback; Pause, Prepare and Seek show a
paused intent. This remains visible through lagging snapshots, acknowledgements,
disconnects and definitive rejections. The `intents` list retains every command
and its error, including earlier commands blocking later work. The latest
preview alone is not sufficient to present those errors.

If a remote action replaces the occurrence, the intent remains visible with
`applies_to_current: false`. It never changes that replacement's transport or
selects the old recording again. The confirmed timeline and readiness remain
separate. None of these display fields grants permission to issue audio-device
commands, bypass readiness, upload media or acquire playback access.

## One sequence and explicit recovery

Queue and transport share the same limit of sixteen intents, one outstanding
packet and one command-ID allocator. `outbound()` returns an unchanged retry
until its receipt is resolved. A confirmed intent retires only after both its
receipt and a confirming snapshot/cursor arrive, in either order. Confirming an
earlier seek cannot erase a later seek's displayed position. Queue projection
does not treat transport actions as advances or removals.

Before first dispatch, a changed occurrence puts the intent into `review` with
`target_changed`. Preparation and start intent also reject a changed media
revision. Play and partial-start intent capture their load generation; a later
preparation requires review rather than silently applying the old intent to the
new load. Fresh readiness reports within the same generation may safely update
the expected room revision before dispatch.

Already sent commands retain their exact ID and payload. A stale revision yields
a durable rejection, with the intent still visible. `retryRejected` requires a
fresh consumed-command cursor and an explicit caller decision. It retains the
original target/load checks. A different load therefore requires discarding the
old rejected/review intent and creating a new explicit intent. Receipt eviction
or a competing device's command ID stays uncertain until explicit reconciliation;
neither causes automatic renumbering or replay.

Preparation and Play are separate explicit protocol commands. Confirm Prepare,
collect real readiness reports for that generation, then enqueue Play or the
owner's explicit partial-start choice. Queuing Play before a new preparation is
confirmed does not automatically retarget it. The model neither synthesizes
Prepare on a Play request nor starts after a deadline. A future native product
flow needs a composite user-intent design while retaining these wire guarantees.

## Validation and remaining gates

Twenty-three new deterministic Windows tests bring the room suite to 153. They
exercise actual SQLite room responses for quick seeks, mixed command sequences,
readiness failures, stale loads/media, competing owner commands, uncertain
receipt eviction, both delivery orders, unchanged reconnect retries, permission
and privacy bounds, and immutability. Fifteen local Durable Object cases pass,
including transport overlay confirmation, reconnect retries and mixed queue/
transport sequencing. The test runtime is disposed afterward.

These are in-memory reconnect models and local runtime calls, not authenticated
WebSockets, process-crash recovery or audible synchronization. A bounded durable
client journal must save intent and the exact assigned packet before any socket
send, restore uncertain outcomes without inventing IDs, and keep private grants
out of its payload. That is the next useful bounded room milestone. Native UI,
composite Play interaction, output/session binding, actual decoder readiness,
clock/output integration and two-computer/Chromecast validation remain absent.
M1 hosted/hardware acceptance remains open.

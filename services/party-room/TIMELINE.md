# Durable timeline spike

The room authority now stores a server-time anchor alongside its queue and
command receipts. This is a backend protocol component, not a working listening
party or an audio engine. It adds no endpoint, infrastructure, secret, provider,
setting or native interface. Run `npm run check` and `npm test` from this service
with Node 24. The optional local runtime command is in [README.md](README.md).

## Anchor and atomicity

Each room snapshot includes `timeline` with exactly these fields:

```json
{
  "occurrence_id": "00000000-0000-4000-8000-000000000101",
  "position_ms": 2000,
  "anchor_at": 1800000000750,
  "playing": true,
  "reason": "playing"
}
```

`anchor_at` is server Unix time in milliseconds. `playing` records the intent to
advance from that anchor, not evidence that a receiver is producing audio. The
existing room epoch and revision order this anchor, queue and media state
together. Every accepted transport command updates one room revision. Its
anchor, revision, command cursor and receipt commit in the same SQLite
transaction and await `storage.sync()` before acknowledgement. Duplicate
commands return their original receipt without resetting the clock.

The additional `room_timeline` table stores one anchor and the greatest observed
authority time. Reopening a current store preserves both; upgrading a queue-only
store creates a paused anchor at its selected occurrence and retains prior queue
receipts. A backward server clock fails closed until it catches up. There is no
wall-clock correction in this module. The separate [clock model](CLOCK.md)
estimates server time with bounded uncertainty; native sampling is still absent.

This is an unreleased schema change: snapshot consumers must understand the
new field. The headless queue client validates it, rejects conflicting anchors
at the same room revision and exposes a copy as `confirmed_timeline`. That field
does not include pending local transport intent. It can refer to the confirmed
current occurrence while an optimistic queue action shows a different selection.
It must not be used to override that pending UI or drive playback directly.

## Owner commands

Use the existing `{epoch, command_id, expected_revision, action}` envelope.
Only the authenticated owner can send transport actions, and each action names
the exact current `occurrence_id`:

| Action | Effect |
| --- | --- |
| `{type: "play", occurrence_id}` | Requires ready media, a position before EOF and the trusted readiness gate. Schedules a start 750 ms after the command's server time. A new start cannot fall at or after room expiry. |
| `{type: "pause", occurrence_id}` | Freezes the projected position immediately, including before a scheduled start. |
| `{type: "seek", occurrence_id, position_ms}` | Accepts an integer from zero through the duration. Cancels the previous start and pauses at that position. Explicit Play must pass readiness again. |

Seeking deliberately pauses in this bounded protocol increment. Seamless
playing seeks require the future load/readiness coordination and optimistic
transport overlay. The existing native player's seek behavior is unchanged.
At EOF the owner must seek before starting again. Selecting another occurrence
always starts with a paused zero-position anchor. There is no automatic advance.

The queue and transport share the same command-ID sequence and receipt window.
Do not run independent command-ID allocators for them. The headless queue client
still enqueues only add/remove/advance; native optimistic transport and a unified
outbox remain implementation gates. Stale revisions, wrong occurrences and
non-owner controls cannot move the anchor. Seek positions participate in command
identity, so a retried command cannot change its target position.

## Readiness and media loss

`RoomQueue` accepts an optional synchronous trusted `canStart(context)` callback.
It defaults to `false`. Literal `true` is required; a promise, exception or any
other result refuses the start. The frozen context contains only:

`{epoch, revision, occurrence_id, media_revision, position_ms, server_time}`.

The adapter must read readiness belonging to this exact occurrence and current
load/position, with valid playback access. Participant JSON cannot supply a
readiness approval. The deterministic tests use fake ready decisions; the local
Workers fixture does the same. Those fixtures are not deployable adapters.

The proposed ten-second readiness barrier, bounded participant reports, owner
decision after the deadline and stale-load fencing are **not implemented** here.
Do not configure an unconditional approval in an application. A real readiness
adapter is required before using these commands for playback.

Uploading and unavailable selections stay paused. A newer trusted media event
that makes the current recording unavailable freezes its position in the same
revision as the media change. An old media event cannot undo it. Becoming ready
again never resumes on its own. This applies to shared-media availability, not
one participant's buffering: participant buffering/status commands are not
accepted as global media events. Late or buffering participants must eventually
join the current anchor without stopping everyone; that client behavior remains
unimplemented.

## Projection and remaining gates

`projectTimeline(anchor, current, serverNow)` is deterministic. A paused anchor
stays fixed; a future start holds position; running position advances at 1x and
clamps at the known duration. Its result distinguishes scheduled, playing and
ended. It does not mutate the queue or claim audible synchronization.

A future consumer must use a fresh, bounded-uncertainty server-clock estimate,
freeze during disconnection/sleep, honor room/session expiry, acquire temporary
media access, and validate output position before applying corrections. Slow
durability or transport can deliver the anchor after its scheduled start;
participants must join at the projected position, not move the shared anchor.
The 750 ms lead is a testable protocol default, not a measured device guarantee.

Owner-presence grace and departure pause, handover, authenticated WebSockets,
durable client journaling, optimistic transport controls, readiness coordination,
native clock integration, receiver commands and drift correction remain absent. No
two-computer or Chromecast run has been performed and synchronization remains
unmeasured. Cloud and hardware acceptance for milestone 1 is still open.

## Validation

On Windows, 23 deterministic timeline tests pass alongside the existing 38 room
tests. They exercise actual SQLite transactions and fake clocks: scheduled
start/pause/seek, permission and readiness failures, stale and duplicate commands,
media loss, queue selection, EOF, room expiry, clock rollback, failed writes,
late/lost durability replies, legacy-store initialization and client validation.
Seven local Workers cases also pass using the real SQLite Durable Object API,
including persisted anchors and receipts after core reconstruction. Runtime
disposal completes after the tests. This is not process-crash, cloud-eviction,
network or acoustic validation.

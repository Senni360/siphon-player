# Headless room connection boundary

`src/connection.mjs` provides `RoomConnection`, a reusable boundary around the
existing journaled client and room clock. It fences callbacks and command sends
to one host transport connection, bounds JSON frames and preserves the exact
durable command through backpressure or uncertain sends. It opens no socket and
adds no endpoint, authentication service, timer, native UI or audio behavior.
No infrastructure, secrets, provider capability, upload quota or media-retention
change is required.

Run `npm run check` and `npm test` with Node 24 in this directory. For just the
adapter regressions, use `node --test test/connection.test.mjs`. No new dependency
is needed. This JavaScript component is an isolated protocol implementation, not
a request to embed a browser or JavaScript engine in the native player.

## Host contract

Open the adapter with the same independently trusted room/actor identity and
dedicated journal used by [JournaledRoomClient](JOURNAL.md):

```js
const adapter = await RoomConnection.open({ roomId, actorId }, journal,
  { now: monotonicNow });
const handle = adapter.attach(enqueueToThisTransport);
// Bind this connection's receive callback to handle.receive(encodedFrame).
// Bind its close callback to handle.close().
```

The host must authenticate the actual transport and authorize the room before
attaching it. `attach` does not establish either grant. Each attach represents a
new transport instance, with its own receive/close callbacks. Never forward an
old transport's events through a global "latest handle", or replace callbacks
on a still-open stream while treating delayed old frames as new-connection data.
The old transport must be closed/discarded by the host; the adapter has no socket
handle with which to close it itself.

`enqueueToThisTransport(encoded)` must synchronously attempt insertion into that
exact connection's bounded output queue and return literal `true` or `false`.
`true` means queue acceptance, not server delivery. `false` means backpressure
with no acceptance. Do not await, defer the actual insertion or look up a mutable
"current socket" inside this callback. Promises and arbitrary return values are
refused. A throwing or asynchronous sender may already have queued bytes, so its
outcome remains uncertain and the original packet stays journaled.

One adapter owns one client journal. Close/suspend the previous adapter before
reopening it; SQLite compare-and-swap is not native process ownership, transport
revocation or a substitute for a file lock. The host still owns journal paths,
user-only access, background disk work, orderly flushing and retention.

## Frames and API

Frames are JSON strings with exactly `{kind, payload}`, capped at 64 KiB of
UTF-8, before JSON decoding. A code-unit bound rejects oversized strings before
allocating their UTF-8 copy. Inbound kinds are `snapshot`, `receipt`, `clock` and
`failure`; the existing client/clock validators check their payloads. Failure
payloads have exactly `{command_id, code}` with a positive command ID and one of
the existing fixed recovery codes. Unknown or private error text is refused.
Outbound kinds are `command` and `probe`, carrying the existing narrow schemas.
No actor, credential, path, filename or playback URL is added to a command.
This is an unreleased internal envelope, not a deployed server wire protocol.

| Operation | Behavior |
| --- | --- |
| `attach(enqueue)` | Retires the old handle and returns frozen `receive`/`close` closures bound to this connection. A fresh authoritative snapshot is required. |
| `handle.receive(encoded)` | Returns whether a valid message applied. Old handles return `false` before decoding. Malformed live frames detach and raise a fixed `ConnectionError`. |
| `sendCommand()` | Awaits durable packet persistence, rechecks the exact connection and enqueues synchronously. Concurrent callers share one dispatch. Returns `sent`, `backpressure`, `stale` or `waiting`. |
| `sendProbe()` | Sends a bounded timestamp probe only after a fresh snapshot. Also returns `limited` when the clock's outstanding-probe bound is reached. |
| `enqueue`, `enqueueTransport`, explicit resolution methods | Delegate optimistic intent and outcome handling to the journaled client. They do not send automatically. |
| `flush()` | Saves dirty state, including receipts and snapshots. The host must schedule this after changes and await it on orderly shutdown. |
| `suspend()` | Retires the active handle and invalidates its clock. The host must also freeze output and close/re-establish its real transport. |
| `view()` | Exposes the existing optimistic view/journal status plus attachment, dispatch, fixed error and clock-estimate state. This is not playback authorization. |

Transport errors detach the affected connection and retain pending intent. Old
senders cannot detach or set an error on a replacement connection, including
reentrant replacement during enqueue. Journal failures release no frame and
remain visible in the existing journal status. The host chooses bounded retry
timing; there is no automatic resend loop, background work or unbounded buffer.

An unanswered probe caused by backpressure still consumes one of the clock's
four pending slots until its normal two-second timeout. It cannot grow the
buffer without bound. The host should back off rather than repeatedly call the
sender. A ready clock does not prove decoder readiness, current media access,
session validity, output latency or permission to play.

## Recovery and clock identity

The actual connection handle fences incoming callbacks before they reach a core.
This prevents probe/generation counter reuse in newly created clock objects from
admitting old-connection replies. A fresh snapshot starts clock sampling; cached
journal state never restores a clock or a live connection.

A recoverable server failure can require a fresh snapshot on the **same**
transport. In that case samples are discarded, but the clock object and probe
high-water mark are retained. A regression demonstrated that recreating the
clock in this path reused an outstanding probe ID. Keeping the counter prevents
a pre-resynchronization reply from matching a newly issued probe. A permanent
room block cannot be cleared by receiving another ordinary snapshot.

Clock discontinuity or suspension remains a reason to re-establish the transport
and obtain fresh state. The component does not correct an OS clock, estimate
output latency, schedule a media start or extrapolate audio through a disconnect.

## Validation and remaining gates

Twenty-one new deterministic Windows tests bring the room suite to 239. They
cover snapshot gating, serialized dispatch, backpressure, durability waits,
old handles, uncertain/async sends, malformed and oversized UTF-8 frames,
private-error redaction, input-identity mutation, suspension, epoch quarantine,
clock-counter collisions and journal reopening. One test routes the adapter
through the existing [fault harness](RECONNECT.md), loses an advance receipt,
reconstructs the SQLite authority and verifies an unchanged retry applies once.
The server membership, media and session proofs in these tests are synthetic.

The existing 23 local Durable Object cases and 60 media-service tests also pass;
the local runtime is disposed. No remote transport, production authentication,
native journal/output adapter, two-computer playback, Chromecast or audible
synchronization was tested. M1 cloud/hardware acceptance remains open.

The next bounded integration can coordinate readiness reports and heartbeat/
reconnect scheduling with this boundary, or test an authenticated local socket
adapter after defining its handshake and resource limits. A public endpoint,
cloud provisioning and actual playback still require their separate gates.

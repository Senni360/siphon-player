# Local reconnect and fault rehearsal

This checkpoint combines the existing room components in a test-only,
two-participant transport harness. It does not add a production transport,
authenticate an account, open a socket, play audio or change the native app.
No infrastructure, secret, upload, storage quota or retention change is needed.

Run with Node 24, without installing dependencies:

```sh
cd services/party-room
npm run test:reconnect
```

The fourteen scenarios also run under `npm test`. The reusable fixture is
`test/support/reconnect-harness.mjs`; the scenarios are in
`test/reconnect.test.mjs`. The full room suite now passes 218 tests on Windows.

## What is connected

One real in-memory SQLite authority runs `SessionRoom`. Two distinct client
SQLite stores run `SqliteRoomJournal` and `JournaledRoomClient`, with separate
`RoomClock` instances and synthetic monotonic offsets. The wire serializes JSON
and parses it at delivery, so sender and receiver do not share message objects.
The fixture can drop, duplicate, delay and reorder requests or replies.

The host connection object supplies the actor and session context. No packet
can choose an actor, and a forged actor field fails the existing command schema.
These fixed identities and fake connection proofs are **not authentication**.
Media is synthetic metadata and `canStart` is an injected fake allow callback;
the harness does not validate file bytes, temporary playback access or a decoder.
The room identity and epoch are trusted fixture constants; this is not a real
handshake or epoch-rotation protocol. Client and authority reconstruction use
their existing in-memory SQLite stores, not a process crash or cloud eviction.

The fixture has at most 64 queued frames, a 64 KiB encoded-body limit and a
256-delivery drain limit. It starts no timers, subprocesses or servers and closes
each SQLite handle after its test. It writes no audio, manifest, packet capture
or QA file. Its bounds are test safeguards, not production rate limits.

## Verified failure boundaries

| Scenario | Verified result |
| --- | --- |
| Two independent clients prepare and start | Both confirm the same queue and authoritative anchor. Synthetic clock estimates contain the known server time within their reported uncertainty. |
| Advance applied, acknowledgement lost | Client and authority reconstruction preserve the original packet. Retrying does not advance twice. |
| Duplicate requests and reordered receipts | The server returns the original receipt; a later optimistic seek remains visible until its own confirmation. |
| Receipt saved before its snapshot | Reopening retains the accepted outcome and waits for fresh state rather than replaying it. |
| Old-connection requests or replies | The host refuses delivery to the replacement connection. Original command identity remains recoverable. |
| Disconnect after journal dispatch returns | A final connection check prevents enqueue to a replacement socket. A stale original command is retained for explicit rejection handling. |
| Journal commit succeeded but reply was lost | No frame is released until the identical write is reconciled. Visible intent remains intact. |
| Output replaced while a readiness report is queued | The old callback cannot ready the replacement. A lost Play receipt cannot reset its original timeline anchor. |
| Owner heartbeat lost while peer remains live | Late maintenance pauses at the original last-owner lease expiry plus grace. Returning does not resume automatically. |
| Clock object recreated with matching counters | An old reply is refused before it reaches the new clock instance. Fresh sampling is still required. |
| Receipt window exhausted during partition | The pending outcome becomes explicitly uncertain; no new command ID is invented. |

The standard 23 local Durable Object cases and 60 media-service tests also pass.
Those runtime cases are a separate runner; this harness itself uses Node SQLite.
None of these results measures network timing, audible synchronization, device
latency, Chromecast handoff or playback on two computers.

## Requirements for the future transport adapter

Two boundaries need checks in the host even when the existing cores are used:

1. **Bind replies to the active transport connection before calling a core.**
   Clock generations and probe IDs are instance-local and restart when a new
   `RoomClock` is constructed. A regression first demonstrated acceptance of an
   old reply with matching counters when this host fence was absent. The fixed
   harness rejects the old connection before clock validation. Never relabel an
   old callback with the replacement connection's generation. A freshly restored
   client journal is also not permission to reuse clock samples.
2. **Recheck the connection at actual send time.** `JournaledRoomClient.outbound()`
   protects its durability wait, but the host can yield again after it returns.
   Capture the connection before calling it and require that exact connection to
   remain active when enqueueing the packet. If it changed, retain the journaled
   packet for reconciliation; do not renumber or silently rebase it.

The fixture illustrates those checks but is not a deployable transport adapter.
A real adapter still needs authenticated session establishment/revocation,
serialized dispatch, bounded frame decoding and backpressure, failure mapping,
heartbeat/alarm scheduling, fresh-snapshot/epoch coordination, monotonic
readiness-report cursors and native journal ownership/retention. The harness
reports one readiness update at a time per actor; it does not implement a native
decoder event queue. Output readiness and temporary media access remain separate
from participant claims and clock readiness.

The [headless connection boundary](CONNECTION.md) now implements these fences,
including same-transport clock resynchronization without probe-ID reuse, and
runs through this fault harness. It still opens no socket. Production auth,
native playback, cloud/hardware milestone-1 acceptance and real two-computer/Cast
validation remain open.

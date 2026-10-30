# Bounded room-clock estimation

`src/clock.mjs` is an in-memory protocol model for estimating server time from a
client's monotonic clock. `RoomQueue.sampleClock()` supplies the timestamp-only
authority exchange. Neither opens a socket, schedules timers, sets an OS clock,
loads audio or changes the native application. Run `npm run check` and `npm test`
in this service with Node 24. No credentials or external service are needed.

## Timestamp exchange

The future authenticated transport passes `{epoch, probe_id}` to
`RoomQueue.sampleClock(actor, request)`. The actor comes from the session, not
the request. The exact reply is:

`{room_id, epoch, probe_id, received_at, sent_at}`.

Server timestamps are Unix milliseconds. The first is captured before the
authority transaction; the second follows its durability wait. Membership and
room expiry are checked again after that wait. Clock rollback and storage
failures refuse the exchange with static error codes. No queue state, recording
identity, credentials or arbitrary metadata is returned.

Sampling uses the existing persisted clock floor, but does not increment the
room revision or consume a queue/transport command ID. Probe IDs are a separate
correlation namespace. There is no deployed endpoint or server request-rate limit
yet. The future adapter must authenticate, bound message bytes, rate-limit probes
and account for their Durable Object/SQLite work before deployment.

## Client API

Construct `RoomClock({roomId, now})` with a monotonic millisecond clock. The default
is `performance.now()` for this JavaScript model; tests inject deterministic
clocks. A future native implementation must use the equivalent native monotonic
source without embedding JavaScript or a browser in the player.

| Method | Contract |
| --- | --- |
| `beginConnection(epoch)` | Clears previous samples and returns a new connection generation. Obtain the epoch from a fresh authorized room snapshot. |
| `probe(generation)` | Records local send time and returns `{epoch, probe_id}`, or `null` if sampling is unavailable. At most four probes may be outstanding. |
| `accept(generation, reply)` | Records local receive time and accepts a matching, bounded reply once. Unknown, duplicate and old-connection replies do not add samples. |
| `estimate(generation)` | Returns `status: "ready"` with integer `server_time_ms`, `uncertainty_ms`, `sample_count` and `minimum_rtt_ms`, or only `status: "unavailable"` and a reason. |
| `disconnect(generation)` | Drops samples and pending probes for that connection. Old disconnect callbacks cannot affect a new connection. |
| `suspend()` | Invalidates immediately. Required on native suspend/resume even if its monotonic clock stopped during sleep. |

After disconnect, suspension, an unexplained observation gap, monotonic rollback,
changed epoch or incompatible server-clock samples, obtain a fresh room snapshot
and begin a new connection generation. Do not reuse the old estimate while
collecting new samples. Merely having a ready clock does not authorize playback.

## Bounds and calculation

| Bound | Pilot value |
| --- | --- |
| Samples required / retained | 3 / 8 |
| Outstanding probes | 4 |
| Maximum round trip / server processing | 2,000 ms / 1,000 ms |
| Sample lifetime | Strictly less than 30 seconds |
| Unexplained gap between local observations | More than 5 seconds invalidates |
| Maximum reported uncertainty | 100 ms |
| Timestamp slack / assumed relative drift | 2 ms / 200 ppm |

For local monotonic send/receive times `t0`/`t3` and server receive/send times
`t1`/`t2`, server processing is `t2 - t1` and path round-trip delay is
`(t3 - t0) - (t2 - t1)`. With nonnegative one-way delays, the server-minus-client
clock offset lies between `t2 - t3` and `t1 - t0`. Each interval expands for
timestamp slack and assumed relative clock drift, then expands further as its
sample ages. The estimate uses the intersection of the recent intervals.
Low-delay replies normally supply tighter bounds; a slow reply cannot drag the
estimate outside those bounds. Incompatible intervals invalidate the connection.

The midpoint is rounded to milliseconds. Estimates never move backward within
one connection: if a tighter sample moves the midpoint backward, the previous
estimate is held and that adjustment is included in the reported uncertainty.
Rounding is also charged to uncertainty. If the result exceeds 100 ms, the clock
returns unavailable instead of presenting a precise-looking time. Samples that
age out require replenishment even if local observations remain frequent.

These bounds depend on the stated timestamp and drift assumptions. The 200 ppm
relative-drift allowance is a model parameter, not a measured guarantee for the
supported computers. The code does not assume symmetric network paths, prove
the correctness of a remote clock, measure audio latency, or correct a device's
playback drift. Authenticated timestamp replies remain required.

## Timeline integration and remaining work

With a fresh authorized snapshot and ready clock, a future adapter can pass
`server_time_ms` to `projectTimeline(snapshot.timeline, snapshot.current, time)`.
It must still honor room/session/media expiry, pending optimistic intent and the
readiness gate. An unavailable clock means freeze the last local estimate and
resynchronize; it must not silently extrapolate through sleep or disconnection.
The native adapter, freeze behavior, periodic probes and reconnect loop are not
implemented here. `RoomClient` queue recovery does not drive this clock yet.

The 100 ms clock threshold is only one part of the provisional synchronization
budget. Output latency, buffering, receiver position accuracy and bounded
corrections still need measurement. No two-computer, Chromecast, cloud or
acoustic synchronization claim follows from these tests.

## Validation

Twenty-three deterministic Windows tests cover delayed/asymmetric and reordered
replies, server processing, sample/probe bounds, duplicate and stale messages,
expiry, suspension, backward clocks, incompatible forward/backward server jumps,
monotonic refinement, privacy, authorization changes and failed storage. The
synthetic delay/drift matrix checks that reported uncertainty contains the known
server time under its assumptions, including 250 exchanges with deterministic
jitter and changing bounded relative drift. An integration test feeds actual SQLite room
samples into a projected timeline with synthetic transit delays.

The complete room suite passes 84 tests. Eight local Workers cases pass,
including this timestamp exchange on a real local SQLite Durable Object. The
runtime is disposed after testing. These checks use one Windows computer and
fake clocks; measured real-world synchronization remains **none**.

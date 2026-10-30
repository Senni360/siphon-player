# Startup readiness protocol

The room now coordinates one bounded load attempt for its current shared-local
recording. This is an isolated protocol component. It does not load audio, issue
URLs, verify stored media, connect participants or control a receiver. Existing
Spotify and desktop behavior is unchanged. No new infrastructure, credentials,
dependencies, settings, upload quota or retention change is required.

Run `npm run check` and `npm test` here with Node 24. The optional local Durable
Object runner is described in [README.md](README.md).

## Owner workflow

Commands use the existing `{epoch, command_id, expected_revision, action}`
envelope and receipt window. Queue and transport commands share one sequence.

| Action | Result |
| --- | --- |
| `{type: "prepare", occurrence_id}` | Owner only, with trusted connected presence and ready media. Pauses at the projected current position and creates a new load generation. |
| `{type: "play", occurrence_id}` | A paused recording starts only when every member of that load cohort has a fresh ready report and the independent `canStart` gate approves. |
| `{type: "start_ready", occurrence_id, generation}` | After the ten-second deadline, the owner may explicitly start with ready participants. The owner's own ready report and `canStart` approval are still required. |

Preparation captures the exact occurrence, media revision, position and paused
anchor. A synchronous trusted `readinessParticipants()` callback supplies a
unique cohort of one to eight currently authorized actors, including the owner.
The default empty roster refuses preparation. Participant JSON cannot choose
the roster. An exception, promise, unauthorized actor or oversized roster fails
closed. The future session authority must supply one active output per actor.

The deadline is ten seconds after preparation. An attempt lasts at most sixty
seconds, capped by room expiry. Preparation requires enough room lifetime for
the initial deadline plus the 750 ms start lead. These are fixed spike bounds,
not settings or measured hardware limits.

Timeout never starts playback automatically. The owner can continue waiting,
prepare a fresh attempt, or explicitly start the ready subset. Readiness that
will expire at or before the scheduled start does not qualify. A participant
omitted from a partial start stays silent until a future late-join protocol
validates its output and joins the projected timeline. That protocol is absent.

## Participant reports and trust

`publishReadiness(actor, {epoch, generation, revision, status})` accepts exactly
those fields. The actor comes from the future authenticated session, not the
JSON. Status is `ready`, `buffering` or `unavailable`. Each actor has a monotonic
report revision within one generation. Older reports are ignored, identical
retries are idempotent, and conflicting equal revisions are rejected.
Reports from an old epoch/generation, a nonmember, an actor outside the captured
cohort or an inactive attempt cannot contribute a vote.

A ready report is fresh for ten seconds, capped by attempt expiry. The output
adapter must recheck its decoder, position, clock estimate and temporary access
before publishing a higher revision to renew it. Repeating the same revision
never refreshes the deadline, even after the vote expires. Revoked members lose
their ready vote; restoring membership does not revive an old report.

These are **participant readiness claims, not trusted storage verification**.
They cannot set media state to ready, change duration or media revision, grant
access or provide URLs. The existing synchronous `canStart(context)` gate still
defaults to false. It must independently validate the verified recording and
temporary playback-access requirements for that exact occurrence and position.
Do not use the fixtures' unconditional approvals in a production adapter.

Load generations fence callbacks from a previous preparation. They do not
authenticate sessions or distinguish two outputs belonging to the same actor.
Before connecting this API, the transport must bind reports to the active
authenticated output/session and reject callbacks from superseded sessions.
Reconnection within an active generation must resume its report revision or
request a new preparation. There is no network handler accepting these reports.

## Persistence, maintenance and snapshots

One bounded `room_readiness` SQLite row stores the current attempt, up to eight
report cursors and a generation high-water mark. It uses the room's existing
transaction and revision authority. An accepted preparation, start or report
advances the room revision once. Maintenance batches simultaneous deadline and
freshness changes into one revision and consumes no command ID. All successful
operations await storage durability. Duplicate command receipts preserve the
original generation, deadline and start, including after core reconstruction.

Pause, seek, advance, current-media revision changes and owner-presence
transitions invalidate the attempt without resetting its generation. Preparing
while playing freezes the projected position and requires entirely fresh votes.
Once started, the readiness snapshot describes startup admission, not continuing
health. Late buffering reports cannot pause the global timeline or change media
availability. Continuous health, reconnect recovery and drift correction need
separate output logic.

Every successful room operation performs overdue readiness maintenance before
its own operation. A command carrying a revision from before maintenance receives
a durable stale-revision rejection. `tickReadiness()` also returns
`{changed, revision, next_alarm_at}` for recovery without participant traffic.
An alarm adapter must schedule the earliest readiness, owner-presence and room
expiry deadline. There is no alarm implementation in this checkpoint. A failed
operation rolls its transaction back; a later successful snapshot or tick
reconstructs overdue state from the original deadlines.

Snapshots add `readiness` with generation, phase, occurrence, media revision,
position, deadline, expiry, participant/ready counts, and only the requesting
actor's inclusion/status/next report revision. No roster, path, filename, file
hash, URL or credential is exposed. Phases are `idle`, `collecting`, `timed_out`,
`started` and `expired`. An idle snapshot retains the generation high-water mark.

This is an unreleased snapshot-schema addition. The headless queue client
validates it, rejects stale/conflicting state and exposes `confirmed_readiness`
without discarding pending queue intent. It also models
[pending transport intent](TRANSPORT-CLIENT.md) in the same command sequence.
It has no native interface, durable outbox or decoder integration.
Confirmed readiness can refer to a different occurrence from an optimistic
selection and must not be used to drive audio directly.

## Validation and remaining gates

On Windows, 26 new deterministic SQLite tests bring the room suite to 130.
They cover permission and trust boundaries, duplicates, stale loads, fresh-vote
expiry, explicit partial starts, invalidation, revocation, fixed deadlines,
reconstruction, concurrent reports, rollback/lost acknowledgements and client
intent preservation. Twelve local Durable Object cases pass, including load
timeout reconstruction, expired duplicate reports, partial starts and stale
loads after seeking. The runner disposes its local runtime afterward.

This is not cloud eviction, process-crash or network validation. No real decoder
readiness, two-computer playback, Chromecast timing or audible synchronization
has been measured. M1 hosted/hardware acceptance remains open. Authenticated
transport/session fencing, actual alarms, temporary access validation, native
optimistic transport and real output integration remain required. The headless
transport model is now tested; durable client journaling remains required before
connecting its commands to a socket or playback.

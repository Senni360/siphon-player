# Client journal spike

The room client now has a bounded checkpoint codec, a write-before-dispatch
coordinator and a local SQLite adapter. The coordinator preserves the original
command packet across a restart without treating an old snapshot as a live
connection. This is isolated protocol code. It adds no native interface, socket,
audio integration, infrastructure, account, credential or provider capability.

Run `npm run check` and `npm test` in this directory with Node 24. No package
installation is needed. Tests include a synthetic SQLite file and a child Node
process that exits after committing its packet. They do not open an app library,
send a request or use a real room account. The test removes only its own exact
SQLite files afterward, not any earlier QA directory.

## Checkpoint format

`RoomClient.checkpoint()` returns a version-1 JSON string of at most 64 KiB.
It contains the bound room/actor IDs, confirmed snapshot, up to sixteen pending
intents, exact assigned packet and any receipt, command/intent high-water marks,
connection generation and persistent room block. It includes no file paths,
filenames, playback links, provider credentials, authorization grants or audio.
The queue and transport preview bounds still apply. SQLite has additional page
and rollback-journal overhead beyond the payload limit.

`RoomClient.restore({roomId, actorId}, checkpoint)` requires separately supplied
identity. It validates the version, byte size, exact nested field schemas,
permissions, action/packet agreement, cursor relationships, receipt outcomes and
ordered intent IDs. Unknown versions, malformed/private fields or inconsistent
state fail with a fixed error; contents are never logged or substituted with an
empty client. This detects malformed state, not malicious edits by someone who
can rewrite a valid local journal. The server remains the playback authority.

Restoration retains uncertain, rejected and review states and invalidates the
saved connection generation. It always starts offline. A new connection and
fresh authoritative snapshot are required before a packet can be dispatched.
Cached timeline, readiness, owner presence and positions are display history,
not permission to resume audio or reuse an old clock estimate. The clock model
itself is not persisted.

## Journal coordinator

Use `await JournaledRoomClient.open(identity, journal)` to obtain the wrapper.
It exposes the client's connection, snapshot/receipt, enqueue and explicit
resolution methods, plus `flush()` and asynchronous `outbound()`. It does not
expose the underlying client or call a sender.

Enqueue changes the displayed intent synchronously and marks the checkpoint
dirty. The host must schedule `flush()` after changes and await it during orderly
shutdown. Dirty intent is not claimed to survive a process exit before a save.
`view().journal` distinguishes `dirty`, `saving`, `saved` and `failed`, with a
local storage revision and fixed error code. There is no automatic background
timer in this library.

`await outbound()` assigns or retrieves the original pending packet, persists
that exact packet and all prior intent, and only then returns it. A failed write
returns an error and no packet. Changes arriving during the disk wait are saved
sequentially; if client activity supersedes that dispatch attempt, it returns
`null` and the caller asks again. In particular, a disconnect or a receipt that
retires the command cannot release a packet to the former connection.

Only one write runs at a time. Repeated unchanged retries do not create new
checkpoint revisions. If a write commits but its acknowledgement is lost, the
coordinator retries its original revision and body before saving newer changes.
It never guesses that the write failed, changes the command ID or overwrites a
competing writer. An accepted receipt saved before its confirming snapshot is
also retained across reopening.

The injected storage contract is:

| Method | Required behavior |
| --- | --- |
| `read()` | Return exactly `{revision, checkpoint}`. Revision zero with a null checkpoint means an explicitly initialized empty journal. |
| `write({expected_revision, checkpoint})` | Atomically compare the stored revision, write the whole checkpoint and durably commit before returning the next integer revision. An identical retry of the immediately preceding write returns that same revision. Conflicting content fails. |

The host must not bypass this wrapper with the raw in-memory client's
`outbound()`. It must also bind returned packets to the same active authenticated
session and serialize actual socket dispatch. This library neither authenticates
connections nor implements exclusive native-process ownership of a journal.

## Local SQLite adapter

`SqliteRoomJournal(database, identity, {create: true})` explicitly initializes an
empty dedicated `node:sqlite` `DatabaseSync`. Omitting `create` opens an existing
journal. Missing rows/tables, identity mismatch or invalid payloads are not reset.
Creation refuses an unrelated or already initialized database. Identity checks
precede changes to SQLite's journal/synchronization modes.

The adapter uses rollback journaling, `synchronous=FULL`, one bounded current
checkpoint row and an atomic revision comparison inside `BEGIN IMMEDIATE`.
A stale writer cannot replace newer state. Unknown write acknowledgements are
idempotent only when both revision and exact checkpoint bytes match. The caller
owns opening and closing the database; the adapter chooses no path and deletes
no file. It contains synchronous disk work and must run on a worker/runtime,
never a native UI thread.

On `journal_conflict`, dispatch stops and the unsaved local view remains available.
The host needs an explicit reconciliation flow with the stored state. Do not
force-overwrite the journal or silently discard the unsaved user's intent.
The spike has no reset, merge or deletion API. Native path selection, user-only
file access, single-instance ownership, migrations and retention/cleanup remain
integration gates. The file is not encrypted and still contains private activity
metadata such as opaque recording IDs; keep it out of Git and shared storage.

## Validation and remaining gates

Twenty-two new deterministic Windows tests bring the room suite to 175. They
cover all pending-outcome states, malformed/oversized/foreign checkpoints,
durability waits, rollback before commit, lost write acknowledgements, concurrent
writes, stale writers, receipt/snapshot ordering and restoration without replay.
One test exits a child process immediately after committing to a real SQLite
file, without explicitly closing it or running JavaScript finalizers. The parent
reopens it, retries the original packet and verifies that the server's receipt
ledger applies the command only once. That is a specific process-exit boundary,
not a power-loss, disk-corruption or native-app crash guarantee.

The existing fifteen local Durable Object scenarios also pass. No native
restart, authenticated WebSocket, deployed room, two-computer party, Chromecast
output or audible synchronization was tested. No infrastructure, secrets, upload
quota or media-retention change is required. M1 hosted/hardware acceptance is
still open. The next bounded room increment can bind readiness reports to the
active participant output/session so superseded sessions cannot renew votes,
before introducing any live room endpoint.

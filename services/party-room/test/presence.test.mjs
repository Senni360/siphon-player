import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { RoomQueue, RoomError, ROOM_LIMITS as L } from "../src/queue.mjs";
import { RoomClient, ClientError } from "../src/client.mjs";
import { projectTimeline } from "../src/timeline.mjs";

const uuid = n => `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
const owner = uuid(1), member = uuid(2), roomId = uuid(3), epoch = uuid(4), occurrence = uuid(5), recording = uuid(6);
const play = { type: "play", occurrence_id: occurrence }, pause = { type: "pause", occurrence_id: occurrence };
const error = code => cause => cause instanceof RoomError && cause.code === code;

async function fixture(t, { connected = true, mediaState = "ready", duration = 120000 } = {}) {
  const db = new DatabaseSync(":memory:"); t.after(() => db.close());
  const storage = {
    sql: { exec(query, ...parameters) {
      const statement = db.prepare(query);
      const rows = statement.columns().length ? statement.all(...parameters) : (statement.run(...parameters), []);
      return { toArray: () => rows.map(row => ({ ...row })) };
    } },
    transactionSync(operation) {
      db.exec("BEGIN");
      try { const result = operation(); db.exec("COMMIT"); return result; }
      catch (error) { db.exec("ROLLBACK"); throw error; }
    },
    async sync() {},
  };
  let now = 1_800_000_000_000;
  const f = { db, storage, ready: true, now: () => now, advance: ms => { now += ms; } };
  const options = { roomId, ownerId: owner, epoch, now: () => now, canStart: () => f.ready,
    authorize: actor => [owner, member].includes(actor),
    resolveRecording: actor => actor === owner ? { owner_id: owner, revision: 1, state: mediaState, duration_ms: duration } : null };
  f.restart = () => { f.queue = new RoomQueue(storage, options); }; f.restart();
  f.view = () => f.queue.snapshot(owner);
  f.event = (revision, connected) => f.queue.publishOwnerPresence({ epoch, revision, connected });
  f.packet = async (action, actor = owner) => {
    const view = await f.queue.snapshot(actor);
    return { epoch, command_id: view.self.next_command_id, expected_revision: view.revision, action };
  };
  f.send = async (action, actor = owner) => f.queue.command(actor, await f.packet(action, actor));
  f.select = async () => {
    await f.send({ type: "add", occurrence_id: occurrence, recording_id: recording, explicit_selection: true });
    await f.send({ type: "advance" });
  };
  f.start = async () => { await f.select(); assert.equal((await f.send(play)).accepted, true); f.advance(1000); };
  if (connected) await f.event(1, true);
  return f;
}

test("unknown owner presence fails closed even when the readiness callback approves", async t => {
  const f = await fixture(t, { connected: false }); await f.select();
  assert.deepEqual((await f.view()).owner_presence, { state: "unknown", pause_at: null, paused: false });
  assert.equal((await f.send(play)).error, "owner_unavailable");
  await f.event(1, true); assert.equal((await f.send(play)).accepted, true);
  assert.deepEqual((await f.view()).owner_presence, { state: "connected", pause_at: null, paused: false });
});

test("trusted presence transitions use room revisions without consuming owner command IDs", async t => {
  const f = await fixture(t, { connected: false }), before = await f.view();
  const result = await f.event(1, true), after = await f.view();
  assert.equal(result.applied, true); assert.equal(result.changed, true);
  assert.equal(after.revision, before.revision + 1); assert.deepEqual(after.self, before.self);
  assert.equal((await f.event(2, true)).changed, false);
  assert.deepEqual(await f.view(), after);
  assert.equal((await f.event(1, false)).applied, false);
  await assert.rejects(f.event(2, false), error("presence_revision_conflict"));
  assert.deepEqual(await f.view(), after);
});

test("grace runs until its exact boundary then pauses once at that deadline", async t => {
  const f = await fixture(t); await f.start();
  const started = await f.view(), lostAt = f.now(), result = await f.event(2, false);
  assert.equal(result.next_alarm_at, lostAt + L.ownerGraceMs);
  f.advance(L.ownerGraceMs - 1);
  let view = await f.view(); assert.equal(view.timeline.playing, true); assert.equal(view.owner_presence.paused, false);
  const revision = view.revision; f.advance(1);
  assert.deepEqual(await f.queue.tickOwnerPresence(), { changed: true, revision: revision + 1, next_alarm_at: null });
  view = await f.view();
  assert.equal(view.timeline.reason, "owner_departed"); assert.equal(view.timeline.playing, false);
  assert.equal(view.timeline.position_ms, lostAt + L.ownerGraceMs - started.timeline.anchor_at);
  assert.equal(view.timeline.anchor_at, lostAt + L.ownerGraceMs); assert.equal(view.owner_presence.paused, true);
  assert.equal((await f.queue.tickOwnerPresence()).changed, false);
  f.advance(10000); assert.equal(projectTimeline((await f.view()).timeline, view.current, f.now()).position_ms, view.timeline.position_ms);
});

test("late timers reconstruct the original deadline instead of adding playback time", async t => {
  const f = await fixture(t); await f.start(); await f.event(2, false);
  const before = await f.view(); f.advance(L.ownerGraceMs + 8000); f.restart();
  assert.equal(f.queue.timeline().playing, true);
  const tick = await f.queue.tickOwnerPresence(), after = await f.view();
  assert.equal(tick.changed, true); assert.equal(after.revision, before.revision + 1);
  assert.equal(after.timeline.anchor_at, before.owner_presence.pause_at);
  assert.equal(after.timeline.position_ms, before.owner_presence.pause_at - before.timeline.anchor_at);
  assert.deepEqual(after.self, before.self); assert.deepEqual(after.pending, before.pending);
});

test("snapshots and clock samples recover overdue presence without a timer", async t => {
  for (const trigger of [f => f.view(), f => f.queue.sampleClock(owner, { epoch, probe_id: 1 })]) {
    const f = await fixture(t); await f.start(); await f.event(2, false);
    const before = await f.view(); f.advance(L.ownerGraceMs + 1); await trigger(f);
    const after = await f.view(); assert.equal(after.timeline.playing, false);
    assert.equal(after.revision, before.revision + 1); assert.deepEqual(after.self, before.self);
  }
});

test("a command based on pre-deadline state is rejected after the timeout revision", async t => {
  const f = await fixture(t); await f.start(); await f.event(2, false);
  const packet = await f.packet(pause); f.advance(L.ownerGraceMs);
  const result = await f.queue.command(owner, packet);
  assert.equal(result.error, "stale_revision"); assert.equal(result.revision, packet.expected_revision + 1);
  assert.deepEqual(await f.queue.command(owner, packet), result);
  assert.equal((await f.view()).timeline.reason, "owner_departed");
});

test("reconnecting before the deadline cancels it without interrupting the running anchor", async t => {
  const f = await fixture(t); await f.start(); const anchor = (await f.view()).timeline;
  await f.event(2, false); f.advance(L.ownerGraceMs - 1); await f.event(3, true);
  const returned = await f.view(); assert.deepEqual(returned.timeline, anchor);
  assert.equal(returned.owner_presence.pause_at, null);
  f.advance(10000); assert.equal((await f.queue.tickOwnerPresence()).changed, false);
  assert.equal((await f.event(2, false)).applied, false);
  assert.equal((await f.view()).timeline.playing, true);
});

test("returning at or after the deadline commits the pause before reconnect and requires explicit resume", async t => {
  for (const lateness of [0, 5000]) {
    const f = await fixture(t); await f.start(); await f.event(2, false);
    const before = await f.view(); f.advance(L.ownerGraceMs + lateness);
    await f.event(3, true); const returned = await f.view();
    assert.equal(returned.revision, before.revision + 2); assert.equal(returned.timeline.playing, false);
    assert.equal(returned.timeline.reason, "owner_departed"); assert.equal(returned.owner_presence.state, "connected");
    f.ready = false; assert.equal((await f.send(play)).error, "readiness_required");
    assert.equal((await f.view()).timeline.playing, false);
    f.ready = true; assert.equal((await f.send(play)).accepted, true);
    assert.equal((await f.view()).timeline.position_ms, returned.timeline.position_ms);
  }
});

test("new and duplicate disconnected reports cannot extend a grace period", async t => {
  const f = await fixture(t); await f.start(); await f.event(2, false);
  const before = await f.view(); f.advance(10000);
  const result = await f.event(3, false); assert.equal(result.applied, true); assert.equal(result.changed, false);
  assert.deepEqual(await f.view(), before);
  assert.equal((await f.event(3, false)).applied, false);
  await assert.rejects(f.event(3, true), error("presence_revision_conflict"));
  assert.equal((await f.event(1, true)).applied, false);
  f.advance(20000); await f.queue.tickOwnerPresence();
  assert.equal((await f.view()).owner_presence.paused, true);
});

test("new starts are refused during grace while existing playback may continue", async t => {
  const f = await fixture(t); await f.start(); await f.event(2, false);
  const anchor = (await f.view()).timeline;
  assert.equal((await f.send(play)).error, "owner_unavailable");
  assert.deepEqual((await f.view()).timeline, anchor);
  assert.equal((await f.send(pause)).accepted, true);
  assert.equal((await f.view()).timeline.playing, false);
});

test("paused, unavailable and empty selections remain valid when grace expires", async t => {
  const paused = await fixture(t); await paused.start(); await paused.send(pause);
  const position = (await paused.view()).timeline.position_ms;
  await paused.event(2, false); paused.advance(L.ownerGraceMs); await paused.queue.tickOwnerPresence();
  assert.equal((await paused.view()).timeline.position_ms, position);
  for (const state of ["uploading", "unavailable"]) {
    const f = await fixture(t, { mediaState: state }); await f.select(); await f.event(2, false);
    f.advance(L.ownerGraceMs); await f.queue.tickOwnerPresence();
    assert.equal((await f.view()).timeline.reason, state);
  }
  const empty = await fixture(t); await empty.event(2, false); empty.advance(L.ownerGraceMs);
  await empty.queue.tickOwnerPresence(); assert.equal((await empty.view()).timeline.reason, "empty");
});

test("departure position clamps at EOF without advancing the queue", async t => {
  const f = await fixture(t, { duration: 10000 }); await f.start(); await f.event(2, false);
  f.advance(L.ownerGraceMs); await f.queue.tickOwnerPresence(); const view = await f.view();
  assert.equal(view.timeline.position_ms, 10000); assert.equal(view.current.id, occurrence);
});

test("room expiry bounds the grace period and remains the final read boundary", async t => {
  const f = await fixture(t); f.advance(L.lifetimeMs - 5000);
  const before = await f.view(), loss = await f.event(2, false);
  assert.equal(loss.next_alarm_at, before.expires_at);
  f.advance(5000); await assert.rejects(f.queue.tickOwnerPresence(), error("room_expired"));
  await assert.rejects(f.view(), error("room_expired"));
});

test("lost presence acknowledgements retain their original deadline over reconstruction", async t => {
  const f = await fixture(t); await f.start(); const deadline = f.now() + L.ownerGraceMs;
  f.storage.sync = async () => { throw new Error("PRIVATE session authority"); };
  await assert.rejects(f.event(2, false), error("state_unavailable"));
  f.storage.sync = async () => {}; f.advance(5000); f.restart();
  const retried = await f.event(2, false);
  assert.equal(retried.applied, false); assert.equal(retried.next_alarm_at, deadline);
});

test("failed timeout storage rolls back anchor, presence and revision together", async t => {
  const f = await fixture(t); await f.start(); await f.event(2, false);
  const before = await f.view(); f.advance(L.ownerGraceMs);
  const exec = f.storage.sql.exec;
  f.storage.sql.exec = (query, ...args) => {
    if (query.startsWith("UPDATE room_owner_presence SET paused=1")) throw new Error("PRIVATE disk details");
    return exec(query, ...args);
  };
  await assert.rejects(f.queue.tickOwnerPresence(), error("state_unavailable"));
  f.storage.sql.exec = exec;
  assert.deepEqual(f.queue.timeline(), before.timeline); assert.equal(f.queue.room().revision, before.revision);
  assert.equal(f.queue.publicPresence().paused, false);
  assert.equal((await f.queue.tickOwnerPresence()).changed, true);
});

test("lost timeout acknowledgements cannot pause a second time after reconstruction", async t => {
  const f = await fixture(t); await f.start(); await f.event(2, false); f.advance(L.ownerGraceMs);
  f.storage.sync = async () => { throw new Error("PRIVATE storage response"); };
  await assert.rejects(f.queue.tickOwnerPresence(), error("state_unavailable"));
  const anchor = f.queue.timeline(), revision = f.queue.room().revision;
  f.storage.sync = async () => {}; f.advance(5000); f.restart();
  assert.deepEqual(await f.queue.tickOwnerPresence(), { changed: false, revision, next_alarm_at: null });
  assert.deepEqual((await f.view()).timeline, anchor);
});

test("malformed, stale-epoch and participant presence claims cannot change the owner", async t => {
  const f = await fixture(t); await f.start(); const before = await f.view();
  for (const event of [{ epoch, revision: 2, connected: "true" }, { epoch, revision: -1, connected: false }]) {
    await assert.rejects(f.queue.publishOwnerPresence(event), error("invalid_presence"));
  }
  await assert.rejects(f.queue.publishOwnerPresence({ epoch: uuid(99), revision: 2, connected: false }), error("stale_epoch"));
  await assert.rejects(f.queue.publishOwnerPresence({ epoch, revision: 2, connected: false, session: "PRIVATE" }), error("invalid_fields"));
  await assert.rejects(f.send({ type: "owner_presence", connected: false }, member), error("invalid_action"));
  assert.deepEqual(await f.view(), before);
  await f.event(2, false); f.advance(L.ownerGraceMs); await f.queue.tickOwnerPresence();
  assert.equal((await f.send(play, member)).error, "forbidden"); assert.equal((await f.view()).owner_id, owner);
});

test("clock rollback cannot restart or postpone a persisted deadline", async t => {
  const f = await fixture(t); await f.start(); await f.event(2, false);
  f.advance(1000); const view = await f.view(); f.advance(-1); f.restart();
  await assert.rejects(f.queue.tickOwnerPresence(), error("invalid_clock"));
  f.advance(1); assert.equal((await f.view()).owner_presence.pause_at, view.owner_presence.pause_at);
});

test("legacy playing stores stop at preserved position and need fresh trusted presence", async t => {
  const f = await fixture(t); await f.select(); const packet = await f.packet(play), receipt = await f.queue.command(owner, packet);
  f.advance(5000); const previousRevision = f.queue.room().revision;
  f.db.exec("DROP TABLE room_owner_presence"); f.restart();
  const view = await f.view(); assert.equal(view.revision, previousRevision + 1);
  assert.equal(view.owner_presence.state, "unknown"); assert.equal(view.timeline.playing, false);
  assert.equal(view.timeline.position_ms, 4250); assert.equal(view.timeline.reason, "owner_departed");
  assert.deepEqual(await f.queue.command(owner, packet), receipt);
  assert.equal((await f.send(play)).error, "owner_unavailable");
  f.advance(-1); await assert.rejects(f.view(), error("invalid_clock")); f.advance(1);
  await f.event(1, true); assert.equal((await f.send(play)).accepted, true);
});

test("queue clients validate confirmed presence without dropping pending intent", async t => {
  const f = await fixture(t); await f.start();
  const client = new RoomClient({ roomId, actorId: owner }), generation = client.beginConnection();
  const connected = await f.view(); client.receiveSnapshot(generation, connected);
  client.enqueue({ type: "advance" });
  await f.event(2, false); const disconnected = await f.view(); client.receiveSnapshot(generation, disconnected);
  assert.equal(client.view().current, null); assert.equal(client.view().intents.length, 1);
  assert.equal(client.view().confirmed_owner_presence.state, "disconnected");
  assert.equal(client.receiveSnapshot(generation, connected), false);
  for (const presence of [{ state: "connected", pause_at: f.now(), paused: false },
    { state: "disconnected", pause_at: null, paused: false },
    { state: "disconnected", pause_at: disconnected.owner_presence.pause_at, paused: true },
    { ...disconnected.owner_presence, session: "PRIVATE" }]) {
    assert.throws(() => client.receiveSnapshot(generation, { ...disconnected, owner_presence: presence }), ClientError);
  }
  assert.throws(() => client.receiveSnapshot(generation, { ...disconnected, owner_presence: connected.owner_presence }),
    cause => cause.code === "snapshot_conflict");
  f.advance(L.ownerGraceMs); const paused = await f.view(); client.receiveSnapshot(generation, paused);
  assert.equal(client.view().confirmed_owner_presence.paused, true); assert.equal(client.view().intents.length, 1);
  const copy = client.view(); copy.confirmed_owner_presence.state = "PRIVATE";
  assert.equal(client.view().confirmed_owner_presence.state, "disconnected");
});

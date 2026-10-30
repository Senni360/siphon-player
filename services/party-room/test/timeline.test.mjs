import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { RoomQueue, RoomError, ROOM_LIMITS } from "../src/queue.mjs";
import { RoomClient, ClientError } from "../src/client.mjs";
import { TIMELINE_LIMITS, projectTimeline, validateTimeline, TimelineError } from "../src/timeline.mjs";

const uuid = n => `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
const owner = uuid(1), member = uuid(2), roomId = uuid(3), epoch = uuid(4), recording = uuid(5);
const occurrence = n => uuid(100 + n);
const add = n => ({ type: "add", occurrence_id: occurrence(n), recording_id: recording, explicit_selection: true });
const transport = (type, n = 1, position) => ({ type, occurrence_id: occurrence(n),
  ...(position === undefined ? {} : { position_ms: position }) });
const error = code => value => value instanceof RoomError && value.code === code;

async function fixture(t, { state = "ready", canStart = () => true } = {}) {
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
  const members = new Set([owner, member]);
  const options = { roomId, ownerId: owner, epoch, now: () => now, canStart,
    authorize: actor => members.has(actor),
    resolveRecording: actor => actor === owner ? { owner_id: owner, revision: 1, state, duration_ms: 12000 } : null };
  const f = { db, storage, options, members, now: () => now, advance: ms => { now += ms; } };
  f.restart = () => { f.queue = new RoomQueue(storage, options); };
  f.restart();
  await f.queue.publishOwnerPresence({ epoch, revision: 1, connected: true });
  f.view = () => f.queue.snapshot(owner);
  f.packet = async (action, actor = owner) => {
    const view = await f.queue.snapshot(actor);
    return { epoch, command_id: view.self.next_command_id, expected_revision: view.revision, action };
  };
  f.send = async (action, actor = owner) => f.queue.command(actor, await f.packet(action, actor));
  f.select = async () => { await f.send(add(1)); await f.send({ type: "advance" }); };
  f.project = async () => { const view = await f.view(); return projectTimeline(view.timeline, view.current, now); };
  f.publish = (revision, state) => f.queue.publishMedia(recording, { owner_id: owner, revision, state, duration_ms: 12000 });
  return f;
}

test("empty and selected timelines stay paused and expose only anchor metadata", async t => {
  const f = await fixture(t);
  let view = await f.view();
  assert.deepEqual(view.timeline, { occurrence_id: null, position_ms: 0, anchor_at: f.now(), playing: false, reason: "empty" });
  await f.select(); view = await f.view();
  assert.equal(view.timeline.reason, "selected"); assert.equal(view.timeline.occurrence_id, occurrence(1));
  f.advance(5000); assert.equal((await f.project()).position_ms, 0);
  assert.doesNotMatch(JSON.stringify(view.timeline), /path|token|url|sha256|filename/);
});

test("authorized Play schedules one fixed start and projections clamp at EOF without advancing", async t => {
  const f = await fixture(t); await f.select(); await f.send(add(2));
  const started = f.now(), result = await f.send(transport("play"));
  assert.equal(result.accepted, true);
  const scheduled = await f.view();
  assert.equal(scheduled.timeline.anchor_at, started + TIMELINE_LIMITS.startLeadMs);
  assert.deepEqual(await f.project(), { occurrence_id: occurrence(1), position_ms: 0, playing: false, scheduled: true, ended: false });
  f.advance(TIMELINE_LIMITS.startLeadMs); assert.equal((await f.project()).playing, true);
  f.advance(2500); assert.equal((await f.project()).position_ms, 2500);
  f.advance(20000);
  assert.deepEqual(await f.project(), { occurrence_id: occurrence(1), position_ms: 12000, playing: false, scheduled: false, ended: true });
  const ended = await f.view();
  assert.equal(ended.current.id, occurrence(1)); assert.equal(ended.pending[0].id, occurrence(2));
  assert.equal(ended.revision, scheduled.revision);
  assert.equal((await f.send(transport("play"))).error, "seek_required");
});

test("Pause freezes elapsed position and another Play resumes it after a fresh lead", async t => {
  const f = await fixture(t); await f.select(); await f.send(transport("play"));
  f.advance(TIMELINE_LIMITS.startLeadMs + 3000);
  await f.send(transport("pause"));
  assert.equal((await f.view()).timeline.reason, "owner_paused");
  f.advance(9000); assert.equal((await f.project()).position_ms, 3000);
  await f.send(transport("play")); f.advance(TIMELINE_LIMITS.startLeadMs + 1000);
  assert.equal((await f.project()).position_ms, 4000);
});

test("pause before a scheduled start cancels it without subtracting lead time", async t => {
  const f = await fixture(t); await f.select(); await f.send(transport("seek", 1, 5000));
  await f.send(transport("play")); f.advance(100); await f.send(transport("pause"));
  f.advance(5000);
  const projection = await f.project(); assert.equal(projection.position_ms, 5000); assert.equal(projection.playing, false);
});

test("seeks are occurrence-bound, pause the anchor and require readiness before resuming", async t => {
  let ready = true;
  const f = await fixture(t, { canStart: () => ready }); await f.select(); await f.send(transport("play"));
  f.advance(1000); ready = false;
  assert.equal((await f.send(transport("seek", 1, 7000))).accepted, true);
  assert.equal((await f.view()).timeline.reason, "seeked");
  assert.equal((await f.send(transport("play"))).error, "readiness_required");
  f.advance(3000); assert.equal((await f.project()).position_ms, 7000);
  const before = (await f.view()).timeline;
  assert.equal((await f.send(transport("seek", 1, 12001))).error, "invalid_position");
  assert.deepEqual((await f.view()).timeline, before);
  assert.equal((await f.send(transport("seek", 2, 0))).error, "wrong_occurrence");
  await f.send(transport("seek", 1, 12000)); assert.equal((await f.project()).ended, true);
  await f.send(transport("seek", 1, 0)); ready = true;
  assert.equal((await f.send(transport("play"))).accepted, true);
});

test("default, asynchronous and failed readiness adapters cannot start a recording", async t => {
  for (const canStart of [undefined, () => false, () => Promise.resolve(true), () => { throw new Error("PRIVATE credential"); }]) {
    const f = await fixture(t); delete f.options.canStart;
    if (canStart) f.options.canStart = canStart;
    f.restart(); await f.select();
    const receipt = await f.send(transport("play"));
    assert.equal(receipt.error, "readiness_required");
    assert.doesNotMatch(JSON.stringify(receipt), /PRIVATE|credential/);
    assert.equal((await f.view()).timeline.playing, false);
  }
});

test("readiness receives exact server-owned occurrence, revision and position, never caller claims", async t => {
  let request;
  const f = await fixture(t, { canStart: value => { request = value; return true; } });
  await f.select(); await f.send(transport("seek", 1, 4321));
  const before = await f.view(); await f.send(transport("play"));
  assert.deepEqual(request, { epoch, revision: before.revision, occurrence_id: occurrence(1),
    media_revision: 1, position_ms: 4321, server_time: f.now() });
  assert.equal(Object.isFrozen(request), true);
  await assert.rejects(f.send({ ...transport("play"), ready: true }), error("invalid_fields"));
});

test("members cannot control transport and revoked owners cannot retrieve old receipts", async t => {
  const f = await fixture(t); await f.select();
  const before = await f.view();
  for (const action of [transport("play"), transport("pause"), transport("seek", 1, 2000)]) {
    assert.equal((await f.send(action, member)).error, "forbidden");
  }
  assert.deepEqual((await f.view()).timeline, before.timeline);
  const packet = await f.packet(transport("play")), accepted = await f.queue.command(owner, packet);
  f.members.delete(owner); await assert.rejects(f.queue.command(owner, packet), error("unavailable"));
  f.members.add(owner); assert.deepEqual(await f.queue.command(owner, packet), accepted);
});

test("duplicates survive reconstruction without restarting elapsed time and seek payloads cannot change", async t => {
  const f = await fixture(t); await f.select();
  const packet = await f.packet(transport("play")), receipt = await f.queue.command(owner, packet);
  const anchor = (await f.view()).timeline;
  f.advance(TIMELINE_LIMITS.startLeadMs + 1000); f.restart();
  assert.deepEqual(await f.queue.command(owner, packet), receipt);
  assert.deepEqual((await f.view()).timeline, anchor);
  assert.equal((await f.project()).position_ms, 1000);
  await f.send(transport("play")); assert.deepEqual((await f.view()).timeline, anchor);
  const seek = await f.packet(transport("seek", 1, 6000)); await f.queue.command(owner, seek);
  await assert.rejects(f.queue.command(owner, { ...seek, action: transport("seek", 1, 6001) }), error("command_conflict"));
});

test("stale commands and stale occurrence targets cannot affect the replacement anchor", async t => {
  const f = await fixture(t); await f.select(); await f.send(add(2));
  const stale = await f.packet(transport("play"), owner);
  // Another owner device consumes this ID, so use the next ID with an old revision.
  await f.send({ type: "advance" }); stale.command_id++;
  const selected = (await f.view()).timeline;
  assert.equal((await f.queue.command(owner, stale)).error, "stale_revision");
  assert.equal((await f.send(transport("pause"))).error, "wrong_occurrence");
  assert.deepEqual((await f.view()).timeline, selected);
});

test("uploading and unavailable selections never start; ready events do not auto-resume", async t => {
  for (const state of ["uploading", "unavailable"]) {
    const f = await fixture(t, { state }); await f.select();
    assert.equal((await f.view()).timeline.reason, state);
    assert.equal((await f.send(transport("play"))).error, "recording_not_ready");
    await f.send(transport("seek", 1, 2000));
    await f.publish(2, "ready");
    const ready = await f.view();
    assert.equal(ready.timeline.playing, false); assert.equal(ready.timeline.position_ms, 2000);
    assert.equal(ready.timeline.reason, "selected");
    assert.equal((await f.send(transport("play"))).accepted, true);
  }
});

test("trusted media loss freezes playing position once and old status cannot resurrect it", async t => {
  const f = await fixture(t); await f.select(); await f.send(transport("play"));
  f.advance(TIMELINE_LIMITS.startLeadMs + 3456);
  const before = await f.view(); await f.publish(3, "unavailable");
  const lost = await f.view();
  assert.equal(lost.revision, before.revision + 1); assert.equal(lost.timeline.position_ms, 3456);
  assert.equal(lost.timeline.reason, "unavailable"); assert.equal(lost.timeline.playing, false);
  f.advance(2000); await f.publish(2, "ready"); assert.deepEqual((await f.view()).timeline, lost.timeline);
  await f.publish(4, "ready"); assert.equal((await f.project()).position_ms, 3456);
  assert.equal((await f.view()).timeline.playing, false);
});

test("adding another occurrence with fresher media status pauses the current recording atomically", async t => {
  const f = await fixture(t); await f.select(); await f.send(transport("play"));
  f.advance(TIMELINE_LIMITS.startLeadMs + 2000);
  f.options.resolveRecording = () => ({ owner_id: owner, revision: 2, state: "unavailable", duration_ms: 12000 });
  f.restart(); const before = await f.view(); await f.send(add(2));
  const after = await f.view();
  assert.equal(after.revision, before.revision + 1); assert.equal(after.timeline.position_ms, 2000);
  assert.equal(after.timeline.reason, "unavailable"); assert.equal(after.pending[0].state, "unavailable");
});

test("advancing a running recording resets the selected anchor and empty queue stays stopped", async t => {
  const f = await fixture(t); await f.select(); await f.send(add(2)); await f.send(transport("play"));
  f.advance(2000); await f.send({ type: "advance" });
  assert.equal((await f.view()).timeline.occurrence_id, occurrence(2));
  assert.equal((await f.project()).position_ms, 0); assert.equal((await f.view()).timeline.playing, false);
  await f.send({ type: "advance" }); assert.equal((await f.view()).timeline.reason, "empty");
});

test("storage failure rolls back anchor, revision and command receipt together", async t => {
  const f = await fixture(t); await f.select();
  const packet = await f.packet(transport("play")), before = await f.view(), exec = f.storage.sql.exec;
  f.storage.sql.exec = (query, ...args) => {
    if (query.startsWith("UPDATE room_queue SET revision=")) throw new Error("PRIVATE disk error");
    return exec(query, ...args);
  };
  await assert.rejects(f.queue.command(owner, packet), error("state_unavailable"));
  f.storage.sql.exec = exec;
  assert.deepEqual(await f.view(), before);
  assert.equal((await f.queue.command(owner, packet)).accepted, true);
});

test("durability acknowledgement may arrive late without moving the stored start", async t => {
  const f = await fixture(t); await f.select(); const packet = await f.packet(transport("play"));
  let release, acknowledged = false;
  f.storage.sync = () => new Promise(resolve => { release = resolve; });
  const result = f.queue.command(owner, packet).then(value => { acknowledged = true; return value; });
  await Promise.resolve(); assert.equal(acknowledged, false);
  f.advance(TIMELINE_LIMITS.startLeadMs + 1500);
  release(); assert.equal((await result).accepted, true); f.storage.sync = async () => {};
  assert.equal((await f.project()).position_ms, 1500);
});

test("lost durability acknowledgements preserve the original play receipt and anchor", async t => {
  const f = await fixture(t); await f.select(); const packet = await f.packet(transport("play"));
  f.storage.sync = async () => { throw new Error("PRIVATE storage response"); };
  await assert.rejects(f.queue.command(owner, packet), error("state_unavailable"));
  const anchor = f.queue.timeline(); f.storage.sync = async () => {};
  f.advance(1000); f.restart(); assert.equal((await f.queue.command(owner, packet)).accepted, true);
  assert.deepEqual((await f.view()).timeline, anchor);
});

test("clock rollback and room expiry cannot advance or rewind a persisted timeline", async t => {
  const f = await fixture(t); await f.select(); await f.send(transport("play"));
  f.advance(2000); const before = await f.view(); f.advance(-1); f.restart();
  await assert.rejects(f.view(), error("invalid_clock"));
  f.advance(1); assert.deepEqual(await f.view(), before);
  f.advance(ROOM_LIMITS.lifetimeMs); await assert.rejects(f.view(), error("room_expired"));
});

test("existing queue databases acquire a paused anchor without changing prior command receipts", async t => {
  const f = await fixture(t); await f.send(add(1));
  const packet = await f.packet({ type: "advance" }), receipt = await f.queue.command(owner, packet);
  f.db.exec("DROP TABLE room_timeline"); f.advance(5000); f.restart();
  assert.deepEqual(await f.queue.command(owner, packet), receipt);
  const view = await f.view(); assert.equal(view.timeline.reason, "selected"); assert.equal(view.timeline.playing, false);
  assert.equal(view.timeline.position_ms, 0); assert.equal(view.revision, receipt.revision);
});

test("a new start cannot be scheduled at or beyond room expiry", async t => {
  const f = await fixture(t); await f.select();
  f.advance(ROOM_LIMITS.lifetimeMs - TIMELINE_LIMITS.startLeadMs);
  assert.equal((await f.send(transport("play"))).error, "room_expiring");
  assert.equal((await f.view()).timeline.playing, false);
});

test("client validates confirmed anchors while retaining its own queue intent", async t => {
  const f = await fixture(t); await f.select();
  const client = new RoomClient({ roomId, actorId: owner }), generation = client.beginConnection();
  const initial = await f.view(); client.receiveSnapshot(generation, initial);
  client.enqueue({ type: "advance" });
  await f.send(transport("play")); const playing = await f.view();
  client.receiveSnapshot(generation, playing);
  assert.equal(client.view().current, null); assert.equal(client.view().confirmed_timeline.playing, true);
  assert.equal(client.view().intents.length, 1);
  assert.throws(() => client.enqueue(transport("pause")), cause => cause instanceof ClientError && cause.code === "invalid_action");
  assert.equal(client.receiveSnapshot(generation, initial), false);
  for (const changed of [{ position_ms: 12001 }, { occurrence_id: occurrence(2) }, { url: "PRIVATE" }, { playing: false }]) {
    assert.throws(() => client.receiveSnapshot(generation, { ...playing, timeline: { ...playing.timeline, ...changed } }), ClientError);
  }
  const changed = { ...playing, timeline: { ...playing.timeline, anchor_at: playing.timeline.anchor_at + 1 } };
  assert.throws(() => client.receiveSnapshot(generation, changed), cause => cause.code === "snapshot_conflict");
  const copy = client.view(); copy.confirmed_timeline.position_ms = 9999;
  assert.equal(client.view().confirmed_timeline.position_ms, 0);
});

test("timeline projection rejects malformed values and does not depend on wall-clock timers", async t => {
  const f = await fixture(t); await f.select(); await f.send(transport("play")); const view = await f.view();
  for (const value of [NaN, Infinity, -1, 1.5, "1000"]) {
    assert.throws(() => projectTimeline(view.timeline, view.current, value), TimelineError);
    assert.throws(() => validateTimeline({ ...view.timeline, position_ms: value }, view.current), TimelineError);
  }
  assert.equal(projectTimeline(view.timeline, view.current, view.timeline.anchor_at + 444).position_ms, 444);
  assert.equal(projectTimeline(view.timeline, view.current, view.timeline.anchor_at - 444).position_ms, 0);
  for (const current of [undefined, { ...view.current, id: "PRIVATE path" }, { ...view.current, state: "buffering" }]) {
    assert.throws(() => projectTimeline(view.timeline, current, f.now()), TimelineError);
  }
});

test("participant buffering claims cannot pause the shared recording or change media readiness", async t => {
  const f = await fixture(t); await f.select(); await f.send(transport("play"));
  const before = await f.view();
  await assert.rejects(f.send({ type: "buffering", occurrence_id: occurrence(1) }, member), error("invalid_action"));
  await assert.rejects(f.send({ type: "publish_media", state: "unavailable" }, member), error("invalid_action"));
  assert.deepEqual(await f.view(), before);
});

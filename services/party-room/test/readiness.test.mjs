import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { RoomQueue, RoomError, ROOM_LIMITS } from "../src/queue.mjs";
import { RoomClient, ClientError } from "../src/client.mjs";
import { READINESS_LIMITS as L } from "../src/readiness.mjs";
import { TIMELINE_LIMITS } from "../src/timeline.mjs";

const uuid = n => `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
const owner = uuid(1), member = uuid(2), outsider = uuid(3), roomId = uuid(4), epoch = uuid(5);
const occurrence = uuid(6), recording = uuid(7);
const action = type => ({ type, occurrence_id: occurrence });
const error = code => cause => cause instanceof RoomError && cause.code === code;

async function fixture(t, overrides = {}) {
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
  const options = { roomId, ownerId: owner, epoch, now: () => now,
    authorize: actor => members.has(actor), canStart: () => true,
    readinessParticipants: () => [owner, member],
    resolveRecording: () => ({ owner_id: owner, revision: 1, state: "ready", duration_ms: 120000 }), ...overrides };
  const f = { db, storage, options, members, now: () => now, advance: ms => { now += ms; } };
  f.restart = () => { f.queue = new RoomQueue(storage, options); }; f.restart();
  f.view = (actor = owner) => f.queue.snapshot(actor);
  f.packet = async (operation, actor = owner) => {
    const view = await f.view(actor);
    return { epoch, command_id: view.self.next_command_id, expected_revision: view.revision, action: operation };
  };
  f.send = async (operation, actor = owner) => f.queue.command(actor, await f.packet(operation, actor));
  f.prepare = () => f.send(action("prepare"));
  f.report = async (actor = owner, status = "ready", extra = {}) => {
    const view = await f.view(actor);
    return f.queue.publishReadiness(actor, { epoch, generation: view.readiness.generation,
      revision: view.readiness.self.next_revision ?? 1, status, ...extra });
  };
  f.partial = async () => f.send({ ...action("start_ready"), generation: (await f.view()).readiness.generation });
  f.allReady = async () => { await f.prepare(); await f.report(); await f.report(member); };
  await f.queue.publishOwnerPresence({ epoch, revision: 1, connected: true });
  await f.send({ type: "add", occurrence_id: occurrence, recording_id: recording, explicit_selection: true });
  await f.send({ type: "advance" });
  return f;
}

test("Play needs an explicit owner load attempt and fresh reports from the whole cohort", async t => {
  const f = await fixture(t);
  assert.equal((await f.send(action("play"))).error, "readiness_required");
  assert.equal((await f.prepare()).accepted, true);
  let view = await f.view();
  assert.equal(view.timeline.reason, "preparing"); assert.equal(view.readiness.phase, "collecting");
  assert.equal(view.readiness.participant_count, 2); assert.equal(view.readiness.self.status, "waiting");
  await f.report(); assert.equal((await f.send(action("play"))).error, "readiness_required");
  await f.report(member); assert.equal((await f.send(action("play"))).accepted, true);
  view = await f.view(); assert.equal(view.readiness.phase, "started");
  assert.equal(view.timeline.anchor_at, f.now() + TIMELINE_LIMITS.startLeadMs);
});

test("participant readiness cannot bypass the independent trusted media-access gate", async t => {
  for (const canStart of [undefined, () => false, () => Promise.resolve(true), () => { throw new Error("PRIVATE grant"); }]) {
    const f = await fixture(t, { canStart }); await f.allReady();
    const before = await f.view(), result = await f.send(action("play"));
    assert.equal(result.error, "readiness_required"); assert.doesNotMatch(JSON.stringify(result), /PRIVATE|grant/);
    assert.deepEqual((await f.view()).timeline, before.timeline);
    assert.equal((await f.view()).current.media_revision, 1);
  }
});

test("participant rosters fail closed, are bounded, unique and derived outside the command", async t => {
  for (const roster of [undefined, () => [], () => [member], () => [owner, owner], () => [owner, outsider],
    () => Array(9).fill(owner), () => Promise.resolve([owner]), () => { throw new Error("PRIVATE sessions"); }]) {
    const f = await fixture(t, { readinessParticipants: roster }); const before = await f.view();
    const result = await f.prepare(); assert.equal(result.error, "participants_unavailable");
    assert.deepEqual((await f.view()).readiness, before.readiness); assert.deepEqual((await f.view()).timeline, before.timeline);
  }
  const f = await fixture(t);
  await assert.rejects(f.send({ ...action("prepare"), participants: [owner] }), error("invalid_fields"));
});

test("only the owner prepares or starts and unknown presence or unavailable media cannot load", async t => {
  const f = await fixture(t);
  assert.equal((await f.send(action("prepare"), member)).error, "forbidden");
  await f.allReady();
  assert.equal((await f.send(action("play"), member)).error, "forbidden");
  await f.queue.publishOwnerPresence({ epoch, revision: 2, connected: false });
  assert.equal((await f.prepare()).error, "owner_unavailable");
  assert.equal((await f.view()).readiness.phase, "idle");
  await f.queue.publishOwnerPresence({ epoch, revision: 3, connected: true });
  await f.queue.publishMedia(recording, { owner_id: owner, revision: 2, state: "uploading", duration_ms: 120000 });
  assert.equal((await f.prepare()).error, "recording_not_ready");
});

test("duplicate prepare receipts survive reconstruction without creating a new generation or deadline", async t => {
  const f = await fixture(t), packet = await f.packet(action("prepare"));
  const receipt = await f.queue.command(owner, packet), initial = await f.view();
  f.advance(2000); f.restart();
  assert.deepEqual(await f.queue.command(owner, packet), receipt);
  assert.deepEqual((await f.view()).readiness, initial.readiness);
  await f.prepare(); assert.equal((await f.view()).readiness.generation, initial.readiness.generation + 1);
  assert.equal((await f.view()).readiness.deadline, f.now() + L.waitMs);
});

test("stale load generations, epochs and conflicting report revisions never contribute readiness", async t => {
  const f = await fixture(t); await f.prepare(); await f.prepare();
  await assert.rejects(f.report(owner, "ready", { generation: 1 }), error("stale_load"));
  await assert.rejects(f.report(owner, "ready", { epoch: uuid(99) }), error("stale_epoch"));
  await f.report(owner, "buffering", { revision: 3 }); const view = await f.view();
  assert.equal((await f.report(owner, "ready", { revision: 2 })).applied, false);
  assert.equal((await f.report(owner, "buffering", { revision: 3 })).applied, false);
  await assert.rejects(f.report(owner, "ready", { revision: 3 }), error("readiness_revision_conflict"));
  assert.deepEqual(await f.view(), view);
});

test("duplicate ready acknowledgements cannot extend freshness, including after expiry", async t => {
  const f = await fixture(t); await f.prepare(); await f.report();
  const initial = await f.view(); f.advance(L.freshMs - 1);
  await f.report(owner, "ready", { revision: 1 });
  assert.equal((await f.view()).revision, initial.revision);
  f.advance(1); const tick = await f.queue.tickReadiness();
  assert.equal(tick.changed, true); assert.equal((await f.view()).readiness.self.status, "waiting");
  const expired = await f.view();
  assert.equal((await f.report(owner, "ready", { revision: 1 })).applied, false);
  assert.deepEqual(await f.view(), expired);
  await f.report(); assert.equal((await f.view()).readiness.self.status, "ready");
});

test("readiness must remain fresh through the scheduled start, not only command admission", async t => {
  for (const delta of [L.freshMs - TIMELINE_LIMITS.startLeadMs - 1, L.freshMs - TIMELINE_LIMITS.startLeadMs]) {
    const f = await fixture(t); await f.allReady(); f.advance(delta);
    assert.equal((await f.send(action("play"))).accepted, delta < L.freshMs - TIMELINE_LIMITS.startLeadMs);
  }
});

test("the ten-second deadline never auto-starts and partial start is an explicit owner command", async t => {
  const f = await fixture(t); await f.prepare(); await f.report();
  assert.equal((await f.partial()).error, "readiness_waiting");
  f.advance(L.waitMs - 1); assert.equal((await f.queue.tickReadiness()).changed, false);
  f.advance(1); f.restart(); const tick = await f.queue.tickReadiness();
  assert.equal(tick.changed, true); assert.equal((await f.view()).timeline.playing, false);
  assert.equal((await f.view()).readiness.phase, "timed_out");
  assert.equal((await f.partial()).error, "readiness_required");
  await f.report(); assert.equal((await f.partial()).accepted, true);
  assert.equal((await f.view(member)).readiness.self.status, "waiting");
  assert.equal((await f.view()).readiness.ready_count, 1);
});

test("partial starts fence generation and omit a peer whose readiness expires before start", async t => {
  const f = await fixture(t); await f.prepare(); f.advance(500);
  await f.report(member); f.advance(L.waitMs - 500); await f.report();
  assert.equal((await f.send({ ...action("start_ready"), generation: 2 })).error, "stale_load");
  const packet = await f.packet({ ...action("start_ready"), generation: 1 });
  const result = await f.queue.command(owner, packet); assert.equal(result.accepted, true);
  assert.equal((await f.view(member)).readiness.self.status, "waiting");
  assert.equal((await f.view()).readiness.ready_count, 1);
  await assert.rejects(f.queue.command(owner, { ...packet, action: { ...packet.action, generation: 2 } }), error("command_conflict"));
});

test("seek, pause, skip and media changes invalidate a load without reusing its generation", async t => {
  for (const mutate of [f => f.send({ ...action("seek"), position_ms: 4000 }), f => f.send(action("pause")),
    f => f.send({ type: "advance" }),
    f => f.queue.publishMedia(recording, { owner_id: owner, revision: 2, state: "ready", duration_ms: 120000 }),
    f => f.queue.publishMedia(recording, { owner_id: owner, revision: 2, state: "unavailable", duration_ms: 120000 })]) {
    const f = await fixture(t); await f.allReady(); await mutate(f);
    const view = await f.view(); assert.equal(view.readiness.phase, "idle"); assert.equal(view.readiness.generation, 1);
    await assert.rejects(f.report(owner, "ready", { generation: 1 }), error("load_inactive"));
    assert.equal(view.timeline.playing, false);
  }
});

test("membership revocation removes ready votes and rejoining needs a new report revision", async t => {
  const f = await fixture(t); await f.allReady(); f.members.delete(member);
  assert.equal((await f.send(action("play"))).error, "readiness_required");
  assert.equal((await f.view()).readiness.ready_count, 1);
  await assert.rejects(f.queue.publishReadiness(member, { epoch, generation: 1, revision: 2, status: "ready" }), error("unavailable"));
  f.members.add(member); await f.report(member, "ready", { revision: 1 });
  assert.equal((await f.send(action("play"))).error, "readiness_required");
  await f.report(member); assert.equal((await f.send(action("play"))).accepted, true);
});

test("late joiners cannot join an existing cohort and reports after start cannot pause playback", async t => {
  const f = await fixture(t, { readinessParticipants: () => [owner] }); await f.prepare();
  await assert.rejects(f.report(member), error("not_in_load"));
  await f.report(); await f.send(action("play")); const before = await f.view();
  await assert.rejects(f.report(owner, "buffering"), error("load_inactive"));
  f.advance(20000); assert.equal((await f.queue.tickReadiness()).changed, false);
  assert.deepEqual((await f.view()).timeline, before.timeline);
});

test("attempt lifetime is durable and a new attempt needs fresh votes", async t => {
  const f = await fixture(t); await f.allReady(); f.advance(L.lifetimeMs); f.restart();
  const view = await f.view(); assert.equal(view.readiness.phase, "expired"); assert.equal(view.readiness.ready_count, 0);
  await assert.rejects(f.report(), error("load_inactive"));
  assert.equal((await f.send(action("play"))).error, "readiness_required");
  await f.prepare(); assert.equal((await f.view()).readiness.generation, 2);
  assert.equal((await f.view()).readiness.ready_count, 0);
});

test("room expiry caps an attempt and refuses preparation too close to expiry", async t => {
  const f = await fixture(t); f.advance(ROOM_LIMITS.lifetimeMs - 20000); await f.prepare();
  const view = await f.view(); assert.equal(view.readiness.expires_at, view.expires_at);
  f.advance(10000); assert.equal((await f.prepare()).error, "room_expiring");
  f.advance(10000); await assert.rejects(f.queue.tickReadiness(), error("room_expired"));
});

test("timeout and freshness maintenance share ordered revisions without consuming commands", async t => {
  const f = await fixture(t); await f.allReady(); const before = await f.view();
  const packet = await f.packet(action("play")); f.advance(L.waitMs);
  const result = await f.queue.command(owner, packet);
  assert.equal(result.error, "stale_revision"); assert.equal(result.revision, before.revision + 1);
  assert.deepEqual(await f.queue.command(owner, packet), result);
  assert.equal((await f.queue.tickReadiness()).changed, false);
  const view = await f.view(); assert.equal(view.readiness.ready_count, 0);
  assert.equal(view.self.next_command_id, before.self.next_command_id + 1);
});

test("lost report acknowledgement survives reconstruction and never refreshes the vote", async t => {
  const f = await fixture(t); await f.prepare();
  f.storage.sync = async () => { throw new Error("PRIVATE storage"); };
  await assert.rejects(f.queue.publishReadiness(owner, { epoch, generation: 1, revision: 1, status: "ready" }), error("state_unavailable"));
  f.storage.sync = async () => {}; f.advance(1000); f.restart();
  assert.equal((await f.report(owner, "ready", { revision: 1 })).applied, false);
  f.advance(L.freshMs - 1000); assert.equal((await f.view()).readiness.self.status, "waiting");
});

test("storage failures roll back readiness, timeline and command receipts together", async t => {
  for (const command of ["prepare", "play"]) {
    const f = await fixture(t); if (command === "play") await f.allReady();
    const packet = await f.packet(action(command)), before = await f.view(), exec = f.storage.sql.exec;
    f.storage.sql.exec = (query, ...args) => {
      if (query.startsWith("UPDATE room_queue SET revision=")) throw new Error("PRIVATE disk");
      return exec(query, ...args);
    };
    await assert.rejects(f.queue.command(owner, packet), error("state_unavailable"));
    f.storage.sql.exec = exec; assert.deepEqual(await f.view(), before);
    assert.equal((await f.queue.command(owner, packet)).accepted, true);
  }
});

test("lost prepare acknowledgement returns the original generation and deadline", async t => {
  const f = await fixture(t), packet = await f.packet(action("prepare"));
  f.storage.sync = async () => { throw new Error("PRIVATE disk"); };
  await assert.rejects(f.queue.command(owner, packet), error("state_unavailable"));
  f.storage.sync = async () => {}; const prepared = await f.view(); f.advance(2000); f.restart();
  assert.equal((await f.queue.command(owner, packet)).accepted, true);
  assert.deepEqual((await f.view()).readiness, prepared.readiness);
});

test("malformed and private report fields are rejected without storing caller metadata", async t => {
  const f = await fixture(t); await f.prepare(); const before = await f.view();
  for (const extra of [{ url: "PRIVATE" }, { status: "verified" }, { revision: Number.MAX_SAFE_INTEGER },
    { revision: 0 }, { generation: 0 }, { status: { path: "PRIVATE" } }, { epoch: "PRIVATE" }]) {
    await assert.rejects(f.report(owner, "ready", extra), error("invalid_readiness"));
  }
  assert.deepEqual(await f.view(), before);
  assert.doesNotMatch(JSON.stringify(before.readiness), /token|path|filename|sha256|url|PRIVATE/);
});

test("client retains pending queue intent while validating readiness snapshots and stale revisions", async t => {
  const f = await fixture(t), client = new RoomClient({ roomId, actorId: owner }), generation = client.beginConnection();
  const initial = await f.view(); client.receiveSnapshot(generation, initial); client.enqueue({ type: "advance" });
  await f.prepare(); await f.report(); const prepared = await f.view();
  client.receiveSnapshot(generation, prepared);
  assert.equal(client.view().current, null); assert.equal(client.view().intents.length, 1);
  assert.equal(client.view().confirmed_readiness.ready_count, 1);
  assert.equal(client.receiveSnapshot(generation, initial), false);
  for (const changes of [{ ready_count: 3 }, { occurrence_id: uuid(99) }, { media_revision: 2 }, { url: "PRIVATE" },
    { self: { included: false, status: "ready", next_revision: 2 } }]) {
    assert.throws(() => client.receiveSnapshot(generation, { ...prepared, readiness: { ...prepared.readiness, ...changes } }), ClientError);
  }
  assert.throws(() => client.receiveSnapshot(generation, { ...prepared,
    readiness: { ...prepared.readiness, deadline: prepared.readiness.deadline + 1 } }), cause => cause.code === "snapshot_conflict");
  const copy = client.view(); copy.confirmed_readiness.ready_count = 99;
  assert.equal(client.view().confirmed_readiness.ready_count, 1);
});

test("preparing during playback freezes the projected position and invalidates previous decoder reports", async t => {
  const f = await fixture(t); await f.allReady(); await f.send(action("play"));
  f.advance(TIMELINE_LIMITS.startLeadMs + 2345); await f.prepare();
  const view = await f.view(); assert.equal(view.timeline.position_ms, 2345); assert.equal(view.timeline.playing, false);
  assert.equal(view.readiness.position_ms, 2345); assert.equal(view.readiness.generation, 2);
  await assert.rejects(f.report(member, "ready", { generation: 1 }), error("stale_load"));
  assert.equal((await f.send(action("play"))).error, "readiness_required");
});

test("concurrent peer reports produce one ordered room revision per accepted report", async t => {
  const f = await fixture(t); await f.prepare(); const before = await f.view();
  const results = await Promise.all([owner, member].map(actor => f.queue.publishReadiness(actor,
    { epoch, generation: 1, revision: 1, status: "ready" })));
  assert.deepEqual(results.map(result => result.revision), [before.revision + 1, before.revision + 2]);
  const view = await f.view(); assert.equal(view.readiness.ready_count, 2); assert.deepEqual(view.self, before.self);
});

test("failed timeout maintenance rolls back all votes and phase before retry", async t => {
  const f = await fixture(t); await f.allReady(); const before = await f.view(); f.advance(L.waitMs);
  const exec = f.storage.sql.exec;
  f.storage.sql.exec = (query, ...args) => {
    if (query.startsWith("UPDATE room_queue SET revision=")) throw new Error("PRIVATE disk");
    return exec(query, ...args);
  };
  await assert.rejects(f.queue.tickReadiness(), error("state_unavailable"));
  f.storage.sql.exec = exec;
  assert.deepEqual(f.queue.readiness.public(owner), before.readiness); assert.equal(f.queue.room().revision, before.revision);
  const tick = await f.queue.tickReadiness(); assert.equal(tick.revision, before.revision + 1);
  assert.equal((await f.view()).readiness.phase, "timed_out");
});

test("lost timeout acknowledgement applies once and clock rollback cannot postpone it", async t => {
  const f = await fixture(t); await f.allReady(); f.advance(L.waitMs);
  f.storage.sync = async () => { throw new Error("PRIVATE response"); };
  await assert.rejects(f.queue.tickReadiness(), error("state_unavailable"));
  f.storage.sync = async () => {}; const revision = f.queue.room().revision; f.restart();
  assert.equal((await f.queue.tickReadiness()).changed, false); assert.equal((await f.view()).revision, revision);
  f.advance(-1); await assert.rejects(f.queue.tickReadiness(), error("invalid_clock"));
  f.advance(1); assert.equal((await f.view()).readiness.phase, "timed_out");
});

test("client normalizes JSON key order and refuses regressing readiness cursors or contradictory timeline", async t => {
  const f = await fixture(t); await f.allReady();
  const client = new RoomClient({ roomId, actorId: owner }), generation = client.beginConnection(), view = await f.view();
  client.receiveSnapshot(generation, view);
  const reordered = Object.fromEntries(Object.entries(view.readiness).reverse());
  reordered.self = Object.fromEntries(Object.entries(reordered.self).reverse());
  assert.equal(client.receiveSnapshot(generation, { ...view, readiness: reordered }), true);
  assert.throws(() => client.receiveSnapshot(generation, { ...view, revision: view.revision + 1,
    readiness: { ...view.readiness, self: { ...view.readiness.self, next_revision: 1 } } }), cause => cause.code === "snapshot_conflict");
  assert.throws(() => client.receiveSnapshot(generation, { ...view,
    timeline: { ...view.timeline, playing: true, reason: "playing" } }), ClientError);
  assert.throws(() => client.receiveSnapshot(generation, { ...view,
    readiness: { ...view.readiness, position_ms: view.readiness.position_ms + 1 } }), ClientError);
});

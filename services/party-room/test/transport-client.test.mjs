import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { RoomQueue, ROOM_LIMITS } from "../src/queue.mjs";
import { RoomClient, ClientError, CLIENT_LIMITS } from "../src/client.mjs";

const uuid = n => `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
const owner = uuid(1), member = uuid(2), roomId = uuid(3), epoch = uuid(4), recording = uuid(5);
const occurrence = n => uuid(100 + n);
const action = (type, n = 1, extra = {}) => ({ type, occurrence_id: occurrence(n), ...extra });
const add = n => action("add", n, { recording_id: recording, explicit_selection: true });
const seek = (position, n = 1) => action("seek", n, { position_ms: position });
const error = code => cause => cause instanceof ClientError && cause.code === code;

async function fixture(t) {
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
  const f = { storage, allowed: true, advance: ms => { now += ms; } };
  const options = { roomId, ownerId: owner, epoch, now: () => now, canStart: () => f.allowed,
    readinessParticipants: () => [owner, member], authorize: actor => [owner, member].includes(actor),
    resolveRecording: () => ({ owner_id: owner, revision: 1, state: "ready", duration_ms: 120000 }) };
  f.restart = () => { f.room = new RoomQueue(storage, options); }; f.restart();
  f.send = async operation => {
    const view = await f.room.snapshot(owner);
    return f.room.command(owner, { epoch, command_id: view.self.next_command_id, expected_revision: view.revision, action: operation });
  };
  await f.room.publishOwnerPresence({ epoch, revision: 1, connected: true });
  for (let n = 1; n <= 3; n++) await f.send(add(n));
  await f.send({ type: "advance" });
  f.client = new RoomClient({ roomId, actorId: owner }); f.generation = f.client.beginConnection();
  f.refresh = async () => f.client.receiveSnapshot(f.generation, await f.room.snapshot(owner));
  await f.refresh(); f.initial = await f.room.snapshot(owner);
  f.enqueue = (operation, position = operation.position_ms ?? 0) => f.client.enqueueTransport(operation, { position_ms: position });
  f.deliver = async (snapshotFirst = false) => {
    const packet = f.client.outbound(); assert.ok(packet);
    const receipt = await f.room.command(owner, packet);
    if (snapshotFirst) await f.refresh();
    f.client.receiveReceipt(f.generation, receipt);
    if (!snapshotFirst) await f.refresh();
    return { packet, receipt };
  };
  f.report = async (actor = owner) => {
    const view = await f.room.snapshot(actor);
    return f.room.publishReadiness(actor, { epoch, generation: view.readiness.generation,
      revision: view.readiness.self.next_revision, status: "ready" });
  };
  f.prepare = async () => { await f.send(action("prepare")); await f.report(); await f.report(member); await f.refresh(); };
  return f;
}

test("pause and seek intent stays visible through stale snapshots and either acknowledgement order", async t => {
  for (const operation of [action("pause"), seek(7000)]) for (const snapshotFirst of [false, true]) {
    const f = await fixture(t), id = f.enqueue(operation, operation.position_ms ?? 4000);
    assert.equal(f.client.view().pending_transport.intent_id, id);
    assert.equal(f.client.view().pending_transport.playing, false);
    f.client.receiveSnapshot(f.generation, f.initial);
    const packet = f.client.outbound(), receipt = await f.room.command(owner, packet);
    if (snapshotFirst) await f.refresh(); else f.client.receiveReceipt(f.generation, receipt);
    assert.equal(f.client.view().pending_transport.intent_id, id);
    if (snapshotFirst) f.client.receiveReceipt(f.generation, receipt); else await f.refresh();
    assert.equal(f.client.view().pending_transport, null); assert.equal(f.client.view().intents.length, 0);
    assert.equal(f.client.receiveSnapshot(f.generation, f.initial), false);
  }
});

test("quick successive seeks retain the latest position while an older seek is confirmed", async t => {
  const f = await fixture(t); f.enqueue(seek(1000)); const second = f.enqueue(seek(9000));
  const firstPacket = f.client.outbound(), firstReceipt = await f.room.command(owner, firstPacket);
  await f.refresh(); assert.equal(f.client.view().pending_transport.position_ms, 9000);
  assert.deepEqual(f.client.outbound(), firstPacket);
  f.client.receiveReceipt(f.generation, firstReceipt);
  assert.equal(f.client.view().pending_transport.intent_id, second);
  const { packet } = await f.deliver(); assert.equal(packet.command_id, firstPacket.command_id + 1);
  assert.equal(f.client.view().confirmed_timeline.position_ms, 9000);
  assert.equal(f.client.view().pending_transport, null);
});

test("queue and transport share one outstanding packet and command-ID sequence", async t => {
  const f = await fixture(t); f.client.enqueue({ type: "advance" }); f.enqueue(seek(6000, 2));
  assert.equal(f.client.view().current.id, occurrence(2));
  assert.equal(f.client.view().pending_transport.applies_to_current, true);
  const first = await f.deliver(true), second = await f.deliver();
  assert.equal(first.packet.action.type, "advance"); assert.equal(second.packet.action.type, "seek");
  assert.equal(second.packet.command_id, first.packet.command_id + 1);
  assert.equal(f.client.view().current.id, occurrence(2)); assert.equal(f.client.view().confirmed_timeline.position_ms, 6000);
  assert.deepEqual(f.client.view().pending.map(row => row.id), [occurrence(3)]);
});

test("explicit prepare, decoder reports and Play complete through the same client recovery flow", async t => {
  const f = await fixture(t); f.enqueue(action("prepare"), 1234);
  assert.equal(f.client.view().pending_transport.action, "prepare");
  const prepared = await f.deliver(); assert.equal(prepared.receipt.accepted, true);
  assert.equal(f.client.view().confirmed_readiness.phase, "collecting");
  await f.report(); await f.report(member); await f.refresh();
  f.enqueue(action("play")); assert.equal(f.client.view().pending_transport.playing, true);
  const started = await f.deliver(true); assert.equal(started.receipt.accepted, true);
  const anchor = f.client.view().confirmed_timeline; assert.equal(anchor.playing, true);
  f.advance(5000); f.restart();
  assert.deepEqual(await f.room.command(owner, started.packet), started.receipt);
  assert.deepEqual((await f.room.snapshot(owner)).timeline, anchor);
});

test("Play intent alone cannot manufacture preparation, readiness or playback access", async t => {
  const f = await fixture(t); const id = f.enqueue(action("play"));
  const rejected = await f.deliver(); assert.equal(rejected.receipt.error, "readiness_required");
  assert.equal(f.client.view().pending_transport.intent_id, id);
  assert.equal(f.client.view().pending_transport.playing, true);
  assert.equal(f.client.view().pending_transport.state, "rejected");
  assert.equal(f.client.view().confirmed_timeline.playing, false); assert.equal(f.client.outbound(), null);
  assert.equal((await f.room.snapshot(owner)).readiness.generation, 0);
});

test("explicit partial start keeps rejection visible and retries only with a fresh cursor and user decision", async t => {
  const f = await fixture(t); await f.send(action("prepare")); await f.report(); await f.refresh();
  const id = f.enqueue(action("start_ready", 1, { generation: 1 }));
  const first = await f.deliver(); assert.equal(first.receipt.error, "readiness_waiting");
  f.advance(10000); await f.report(); await f.refresh();
  assert.equal(f.client.outbound(), null); assert.equal(f.client.view().pending_transport.state, "rejected");
  f.client.retryRejected(id); const second = await f.deliver();
  assert.equal(second.receipt.accepted, true); assert.equal(second.packet.command_id, first.packet.command_id + 1);
  assert.equal(f.client.view().confirmed_readiness.ready_count, 1);
});

test("a replacement occurrence never receives queued transport intended for the old track", async t => {
  for (const operation of [action("pause"), seek(5000), action("prepare"), action("play")]) {
    const f = await fixture(t), id = f.enqueue(operation);
    await f.send({ type: "advance" }); await f.refresh();
    assert.equal(f.client.outbound(), null);
    assert.equal(f.client.view().pending_transport.intent_id, id);
    assert.equal(f.client.view().pending_transport.applies_to_current, false);
    assert.equal(f.client.view().pending_transport.error, "target_changed");
    assert.equal(f.client.view().current.id, occurrence(2));
    f.client.discard(id); assert.equal(f.client.view().pending_transport, null);
  }
});

test("competing owner commands keep sent transport uncertain without allocating a new ID", async t => {
  const f = await fixture(t), id = f.enqueue(seek(7000)), packet = f.client.outbound();
  await f.send({ type: "advance" });
  await assert.rejects(f.room.command(owner, packet), { code: "command_conflict" });
  f.client.receiveFailure(f.generation, packet.command_id, "command_conflict");
  await f.refresh(); assert.equal(f.client.view().pending_transport.state, "uncertain");
  assert.equal(f.client.view().pending_transport.applies_to_current, false); assert.equal(f.client.outbound(), null);
  assert.throws(() => f.client.discard(id), error("outcome_unresolved"));
  f.client.acceptUncertainOutcome(id); assert.equal(f.client.view().pending_transport, null);
  assert.equal((await f.room.snapshot(owner)).timeline.position_ms, 0);
});

test("new media revisions require review before unsent preparation or starts", async t => {
  for (const type of ["prepare", "play", "start_ready"]) {
    const f = await fixture(t); await f.prepare();
    f.enqueue(action(type, 1, type === "start_ready" ? { generation: 1 } : {}));
    await f.room.publishMedia(recording, { owner_id: owner, revision: 2, state: "ready", duration_ms: 120000 });
    await f.refresh(); assert.equal(f.client.outbound(), null);
    assert.equal(f.client.view().pending_transport.error, "media_changed");
    assert.equal(f.client.view().pending_transport.state, "review");
  }
});

test("a new preparation cannot silently inherit queued Play or partial-start intent", async t => {
  for (const type of ["play", "start_ready"]) {
    const f = await fixture(t); await f.prepare();
    f.enqueue(action(type, 1, type === "start_ready" ? { generation: 1 } : {}));
    await f.send(action("prepare")); await f.refresh();
    assert.equal(f.client.outbound(), null); assert.equal(f.client.view().pending_transport.error, "load_changed");
    assert.equal(f.client.view().confirmed_readiness.generation, 2);
  }
});

test("fresh reports can rebase an unsent Play within its original load generation", async t => {
  const f = await fixture(t); await f.send(action("prepare")); await f.refresh(); f.enqueue(action("play"));
  await f.report(); await f.report(member); await f.refresh();
  assert.equal((await f.deliver()).receipt.accepted, true);
});

test("a report arriving after dispatch causes a durable rejection, never an automatic Play retry", async t => {
  const f = await fixture(t); await f.prepare(); const id = f.enqueue(action("play")), packet = f.client.outbound();
  await f.report(member); const receipt = await f.room.command(owner, packet);
  f.client.receiveReceipt(f.generation, receipt); assert.equal(receipt.error, "stale_revision");
  assert.throws(() => f.client.retryRejected(id), error("needs_snapshot"));
  await f.refresh(); assert.equal(f.client.outbound(), null);
  assert.equal(f.client.view().pending_transport.playing, true); f.client.retryRejected(id);
  assert.equal((await f.deliver()).receipt.accepted, true);
});

test("evicted seek receipts remain uncertain even when the snapshot resembles the requested position", async t => {
  const f = await fixture(t), id = f.enqueue(seek(7777)), packet = f.client.outbound();
  await f.room.command(owner, packet);
  for (let n = 0; n <= ROOM_LIMITS.receipts; n++) await f.send(action("remove", 99));
  await f.refresh(); assert.equal(f.client.view().pending_transport.position_ms, 7777);
  assert.equal(f.client.view().pending_transport.state, "uncertain"); assert.equal(f.client.outbound(), null);
  f.client.acceptUncertainOutcome(id); assert.equal(f.client.view().pending_transport, null);
  assert.equal(f.client.view().confirmed_timeline.position_ms, 7777);
});

test("reconnect retries the original start and ignores old-connection receipts and snapshots", async t => {
  const f = await fixture(t); await f.prepare(); f.enqueue(action("play")); const packet = f.client.outbound();
  const receipt = await f.room.command(owner, packet), old = f.generation;
  f.client.disconnect(old); assert.equal(f.client.outbound(), null);
  f.generation = f.client.beginConnection();
  assert.equal(f.client.receiveReceipt(old, receipt), false);
  assert.equal(f.client.receiveSnapshot(old, await f.room.snapshot(owner)), false);
  f.advance(5000); f.restart(); await f.refresh(); assert.deepEqual(f.client.outbound(), packet);
  f.client.receiveReceipt(f.generation, await f.room.command(owner, packet));
  assert.equal(f.client.view().pending_transport, null);
  assert.equal((await f.room.snapshot(owner)).timeline.anchor_at, f.client.view().confirmed_timeline.anchor_at);
});

test("transport previews and all intents share privacy, position and memory bounds", async t => {
  const f = await fixture(t);
  for (const preview of [{ position_ms: -1 }, { position_ms: 120001 }, { position_ms: 1.5 },
    { position_ms: 0, url: "PRIVATE" }, { position_ms: 0, path: "PRIVATE" }]) {
    assert.throws(() => f.client.enqueueTransport(action("pause"), preview), ClientError);
  }
  assert.throws(() => f.enqueue(seek(1000), 2000), error("invalid_preview"));
  assert.throws(() => f.enqueue(action("pause", 2)), error("wrong_occurrence"));
  assert.throws(() => f.enqueue(action("start_ready", 1, { generation: 1 })), error("load_changed"));
  assert.throws(() => f.enqueue({ ...action("play"), token: "PRIVATE" }), error("invalid_action"));
  f.client.disconnect(f.generation);
  for (let n = 0; n < CLIENT_LIMITS.intents - 1; n++) f.enqueue(seek(n));
  f.client.enqueue({ type: "advance" });
  assert.throws(() => f.enqueue(action("pause", 2)), error("pending_limit"));
  assert.doesNotMatch(JSON.stringify(f.client.view()), /PRIVATE|token|path|url/);
});

test("membership cannot grant owner transport controls through a local preview", async t => {
  const f = await fixture(t), client = new RoomClient({ roomId, actorId: member }), generation = client.beginConnection();
  client.receiveSnapshot(generation, await f.room.snapshot(member));
  for (const operation of [action("prepare"), action("play"), action("pause"), seek(1000)]) {
    assert.throws(() => client.enqueueTransport(operation, { position_ms: operation.position_ms ?? 0 }), error("forbidden"));
  }
  assert.equal(client.view().pending_transport, null);
});

test("caller objects, returned packets and displayed intent cannot mutate a transport retry", async t => {
  const f = await fixture(t), operation = seek(4567), preview = { position_ms: 4567 };
  f.client.enqueueTransport(operation, preview); operation.position_ms = 9999; preview.position_ms = 9999;
  const packet = f.client.outbound(), expected = structuredClone(packet); packet.action.position_ms = 9999;
  const view = f.client.view(); view.pending_transport.position_ms = 9999; view.intents[0].action.position_ms = 9999;
  assert.deepEqual(f.client.outbound(), expected); assert.equal(f.client.view().pending_transport.position_ms, 4567);
});

test("discarding an unsent later seek restores the earlier pending intent but sent work cannot be cancelled locally", async t => {
  const f = await fixture(t), first = f.enqueue(seek(1000)), second = f.enqueue(seek(8000));
  f.client.discard(second); assert.equal(f.client.view().pending_transport.intent_id, first);
  f.client.outbound(); assert.throws(() => f.client.discard(first), error("outcome_unresolved"));
  await f.deliver(); assert.equal(f.client.view().pending_transport, null);
});

test("access or owner-presence rejection leaves desired playback visible with an explicit error", async t => {
  for (const denied of ["readiness_required", "owner_unavailable"]) {
    const f = await fixture(t); await f.prepare();
    if (denied === "readiness_required") f.allowed = false;
    else { await f.room.publishOwnerPresence({ epoch, revision: 2, connected: false }); await f.refresh(); }
    f.enqueue(action("play")); const result = await f.deliver();
    assert.equal(result.receipt.error, denied); assert.equal(f.client.view().pending_transport.error, denied);
    assert.equal(f.client.view().confirmed_timeline.playing, false);
  }
});

test("displayed pause and prepare positions never become authority claims in the packet", async t => {
  for (const type of ["pause", "prepare"]) {
    const f = await fixture(t); await f.prepare(); await f.send(action("play")); f.advance(3750); await f.refresh();
    f.enqueue(action(type), 9999); const packet = f.client.outbound();
    assert.deepEqual(packet.action, action(type)); assert.equal(f.client.view().pending_transport.position_ms, 9999);
    const receipt = await f.room.command(owner, packet); f.client.receiveReceipt(f.generation, receipt);
    assert.equal(f.client.view().pending_transport.position_ms, 9999);
    await f.refresh(); assert.equal(f.client.view().confirmed_timeline.position_ms, 3000);
  }
});

test("rejected starts cannot be explicitly retried against a different load without review", async t => {
  const f = await fixture(t); await f.prepare(); f.allowed = false; const id = f.enqueue(action("play"));
  await f.deliver(); await f.prepare(); f.allowed = true; f.client.retryRejected(id);
  assert.equal(f.client.outbound(), null); assert.equal(f.client.view().pending_transport.error, "load_changed");
  f.client.discard(id); f.enqueue(action("play")); assert.equal((await f.deliver()).receipt.accepted, true);
});

test("changed epochs quarantine transport intent while expired rooms block dispatch", async t => {
  const f = await fixture(t); f.enqueue(seek(6000)); const packet = f.client.outbound();
  f.client.receiveFailure(f.generation, packet.command_id, "room_expired");
  assert.equal(f.client.outbound(), null); assert.equal(f.client.view().pending_transport.position_ms, 6000);
  const g = await fixture(t); g.enqueue(action("play"));
  assert.equal(g.client.receiveSnapshot(g.generation, { ...g.initial, epoch: uuid(99) }), false);
  assert.equal(g.client.view().blocked, "epoch_changed"); assert.equal(g.client.outbound(), null);
  assert.equal(g.client.view().pending_transport.playing, true);
});

test("conflicting or private error receipts never replace transport intent awaiting confirmation", async t => {
  const f = await fixture(t); f.enqueue(seek(1000)); const packet = f.client.outbound();
  const receipt = await f.room.command(owner, packet); f.client.receiveReceipt(f.generation, receipt);
  assert.throws(() => f.client.receiveReceipt(f.generation, { ...receipt, accepted: false, error: "readiness_required" }), error("receipt_conflict"));
  assert.throws(() => f.client.receiveReceipt(f.generation, { ...receipt, accepted: false, error: "PRIVATE storage" }), error("invalid_message"));
  assert.equal(f.client.view().pending_transport.state, "accepted"); await f.refresh();
  assert.equal(f.client.view().pending_transport, null);
});

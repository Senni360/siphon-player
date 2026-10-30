import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { RoomQueue, ROOM_LIMITS } from "../src/queue.mjs";
import { RoomClient, ClientError, CLIENT_LIMITS } from "../src/client.mjs";

const uuid = n => `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
const owner = uuid(1), member = uuid(2), roomId = uuid(4), epoch = uuid(5);
const occurrence = n => uuid(n + 1000), recording = n => uuid(n + 2000);
const add = (n, track = 1) => ({ type: "add", occurrence_id: occurrence(n), recording_id: recording(track), explicit_selection: true });
const remove = n => ({ type: "remove", occurrence_id: occurrence(n) });
const advance = { type: "advance" };
const preview = (n, actor = owner, track = 1) => ({ id: occurrence(n), recording_id: recording(track),
  added_by: actor, state: "uploading", duration_ms: 12000, media_revision: 1, source: "shared_local" });
const error = code => value => value instanceof ClientError && value.code === code;
const ids = client => client.view().pending.map(row => row.id);

async function fixture(t, seeded = 0) {
  const db = new DatabaseSync(":memory:"); t.after(() => db.close());
  const storage = {
    sql: { exec(query, ...parameters) {
      const statement = db.prepare(query);
      const rows = statement.columns().length ? statement.all(...parameters) : (statement.run(...parameters), []);
      return { toArray: () => rows.map(row => ({ ...row })) };
    } },
    transactionSync(operation) {
      db.exec("BEGIN");
      try { const value = operation(); db.exec("COMMIT"); return value; }
      catch (error) { db.exec("ROLLBACK"); throw error; }
    },
    async sync() {},
  };
  const members = new Set([owner, member]), media = new Map([
    [recording(1), { owner_id: owner, revision: 1, state: "uploading", duration_ms: 12000 }],
    [recording(2), { owner_id: member, revision: 1, state: "uploading", duration_ms: 12000 }],
  ]);
  const room = new RoomQueue(storage, { roomId, ownerId: owner, epoch, now: () => 1_800_000_000_000,
    authorize: actor => members.has(actor), resolveRecording: (actor, id) => media.get(id)?.owner_id === actor ? media.get(id) : null });
  const f = { room, media, members };
  f.send = async (actor, action) => {
    const snapshot = await room.snapshot(actor);
    return room.command(actor, { epoch, command_id: snapshot.self.next_command_id, expected_revision: snapshot.revision, action });
  };
  for (let n = 1; n <= seeded; n++) await f.send(owner, add(n));
  if (seeded) await f.send(owner, advance);
  f.client = new RoomClient({ roomId, actorId: owner });
  f.generation = f.client.beginConnection();
  f.initial = await room.snapshot(owner);
  f.client.receiveSnapshot(f.generation, f.initial);
  f.refresh = async () => f.client.receiveSnapshot(f.generation, await room.snapshot(owner));
  return f;
}

test("queued additions remain visible before sending and through lagging snapshots", async t => {
  const f = await fixture(t), { client } = f;
  const intent = client.enqueue(add(1), preview(1));
  assert.deepEqual(ids(client), [occurrence(1)]);
  assert.equal(client.view().intents[0].state, "queued");
  client.receiveSnapshot(f.generation, f.initial);
  assert.deepEqual(ids(client), [occurrence(1)]);
  const packet = client.outbound();
  assert.deepEqual(client.outbound(), packet);
  assert.equal(client.view().intents[0].intent_id, intent);
  assert.throws(() => client.discard(intent), error("outcome_unresolved"));
});

test("two optimistic advances never double-pop when snapshots arrive before receipts", async t => {
  const f = await fixture(t, 3), { client } = f;
  client.enqueue(advance); client.enqueue(advance);
  assert.equal(client.view().current.id, occurrence(3)); assert.deepEqual(ids(client), []);
  const first = client.outbound(), receipt = await f.room.command(owner, first);
  const intermediate = await f.room.snapshot(owner);
  client.receiveSnapshot(f.generation, intermediate);
  assert.equal(client.view().current.id, occurrence(3));
  assert.deepEqual(client.outbound(), first);
  client.receiveReceipt(f.generation, receipt);
  const second = client.outbound();
  assert.equal(second.command_id, first.command_id + 1);
  assert.equal(second.expected_revision, receipt.revision);
  const secondReceipt = await f.room.command(owner, second);
  client.receiveReceipt(f.generation, secondReceipt);
  assert.equal(client.view().intents[0].state, "accepted");
  assert.equal(client.view().current.id, occurrence(3));
  await f.refresh();
  assert.equal(client.view().intents.length, 0); assert.equal(client.view().current.id, occurrence(3));
  assert.equal(client.receiveSnapshot(f.generation, intermediate), false);
  assert.equal(client.view().current.id, occurrence(3));
});

test("an acknowledged add waits for a confirming snapshot and snapshot-first delivery does not duplicate its row", async t => {
  for (const snapshotFirst of [false, true]) {
    const f = await fixture(t), { client } = f;
    client.enqueue(add(1), preview(1));
    const packet = client.outbound(), receipt = await f.room.command(owner, packet);
    if (snapshotFirst) await f.refresh(); else client.receiveReceipt(f.generation, receipt);
    assert.deepEqual(ids(client), [occurrence(1)]); assert.equal(client.view().intents.length, 1);
    if (snapshotFirst) client.receiveReceipt(f.generation, receipt); else await f.refresh();
    assert.deepEqual(ids(client), [occurrence(1)]); assert.equal(client.view().intents.length, 0);
  }
});

test("lost acknowledgements reconnect using the original packet and ignore the old socket", async t => {
  const f = await fixture(t), { client } = f;
  client.enqueue(add(1), preview(1)); const packet = client.outbound();
  const receipt = await f.room.command(owner, packet);
  const oldGeneration = f.generation;
  client.disconnect(oldGeneration);
  assert.equal(client.outbound(), null); assert.deepEqual(ids(client), [occurrence(1)]);
  assert.equal(client.receiveSnapshot(oldGeneration, await f.room.snapshot(owner)), false);
  assert.equal(client.view().mode, "offline");
  f.generation = client.beginConnection();
  assert.equal(client.receiveReceipt(oldGeneration, receipt), false);
  assert.equal(client.outbound(), null);
  await f.refresh();
  assert.deepEqual(client.outbound(), packet);
  client.receiveReceipt(f.generation, await f.room.command(owner, client.outbound()));
  assert.equal(client.view().intents.length, 0);
  assert.equal((await f.room.snapshot(owner)).pending.length, 1);
});

test("rejection cursors advance without a room revision and stale cursors cannot reuse an ID", async t => {
  const f = await fixture(t), { client } = f;
  const intent = client.enqueue(add(1), preview(1)); const packet = client.outbound();
  const original = f.media.get(recording(1)); f.media.delete(recording(1));
  const rejection = await f.room.command(owner, packet);
  client.receiveReceipt(f.generation, rejection);
  assert.equal(client.view().intents[0].state, "rejected"); assert.deepEqual(ids(client), [occurrence(1)]);
  client.discard(intent); f.media.set(recording(1), original);
  client.enqueue(add(2), preview(2));
  assert.equal(client.outbound(), null);
  await f.refresh();
  assert.equal(client.receiveSnapshot(f.generation, f.initial), false);
  assert.equal(client.outbound().command_id, packet.command_id + 1);
});

test("a stale revision keeps the rejected row visible and retries only after explicit user intent", async t => {
  const f = await fixture(t), { client } = f;
  const intent = client.enqueue(add(1), preview(1)), first = client.outbound();
  await f.send(member, add(2, 2));
  client.receiveReceipt(f.generation, await f.room.command(owner, first));
  assert.throws(() => client.retryRejected(intent), error("needs_snapshot"));
  await f.refresh();
  assert.deepEqual(ids(client), [occurrence(2), occurrence(1)]);
  assert.equal(client.outbound(), null);
  assert.equal(client.view().intents[0].error, "stale_revision");
  client.retryRejected(intent);
  const retry = client.outbound(); assert.notEqual(retry.command_id, first.command_id);
  client.receiveReceipt(f.generation, await f.room.command(owner, retry)); await f.refresh();
  assert.deepEqual(ids(client), [occurrence(2), occurrence(1)]);
  assert.equal(client.view().intents.length, 0);
});

test("an expired receipt remains uncertain and is never automatically renumbered or replayed", async t => {
  const f = await fixture(t, 2), { client } = f;
  const intent = client.enqueue(advance), packet = client.outbound();
  await f.room.command(owner, packet);
  for (let n = 0; n < ROOM_LIMITS.receipts + 1; n++) await f.send(owner, remove(999));
  await f.refresh();
  assert.equal(client.view().intents[0].state, "uncertain"); assert.equal(client.outbound(), null);
  assert.throws(() => client.discard(intent), error("outcome_unresolved"));
  assert.throws(() => client.retryRejected(intent), error("outcome_unresolved"));
  client.acceptUncertainOutcome(intent);
  assert.equal(client.view().intents.length, 0); assert.equal(client.view().current.id, occurrence(2));
});

test("a competing device's command ID cannot silently replace the local intent", async t => {
  const f = await fixture(t), { client } = f;
  const intent = client.enqueue(add(1), preview(1)), packet = client.outbound();
  await f.send(owner, add(2));
  await assert.rejects(f.room.command(owner, packet), { code: "command_conflict" });
  client.receiveFailure(f.generation, packet.command_id, "command_conflict");
  assert.throws(() => client.acceptUncertainOutcome(intent), error("needs_snapshot"));
  await f.refresh();
  assert.deepEqual(ids(client), [occurrence(2), occurrence(1)]); assert.equal(client.outbound(), null);
  client.acceptUncertainOutcome(intent); assert.deepEqual(ids(client), [occurrence(2)]);
});

test("rebasing an unsent advance never skips a different occurrence after the head changed", async t => {
  const f = await fixture(t, 3), { client } = f;
  const intent = client.enqueue(advance);
  await f.send(owner, advance); await f.refresh();
  assert.equal(client.outbound(), null);
  assert.equal(client.view().intents[0].state, "review");
  assert.equal(client.view().intents[0].error, "queue_changed");
  assert.equal(client.view().current.id, occurrence(2));
  client.discard(intent); client.enqueue(advance);
  const packet = client.outbound(); assert.ok(packet);
  client.receiveReceipt(f.generation, await f.room.command(owner, packet)); await f.refresh();
  assert.equal(client.view().current.id, occurrence(3));
});

test("unrelated appends may rebase an unsent advance while keeping its selected head", async t => {
  const f = await fixture(t, 2), { client } = f;
  client.enqueue(advance); await f.send(member, add(3, 2)); await f.refresh();
  const packet = client.outbound(); assert.ok(packet);
  client.receiveReceipt(f.generation, await f.room.command(owner, packet)); await f.refresh();
  assert.equal(client.view().current.id, occurrence(2)); assert.deepEqual(ids(client), [occurrence(3)]);
});

test("optimistic removals stay removed through stale and snapshot-first delivery", async t => {
  const f = await fixture(t, 3), { client } = f;
  client.enqueue(remove(2)); assert.deepEqual(ids(client), [occurrence(3)]);
  client.receiveSnapshot(f.generation, f.initial); assert.deepEqual(ids(client), [occurrence(3)]);
  const receipt = await f.room.command(owner, client.outbound()); await f.refresh();
  assert.deepEqual(ids(client), [occurrence(3)]);
  client.receiveReceipt(f.generation, receipt); assert.equal(client.view().intents.length, 0);
});

test("pending intent and message fields are bounded and contain no hidden private metadata", async t => {
  const f = await fixture(t), { client } = f;
  for (const extra of [{ local_path: "PRIVATE" }, { url: "PRIVATE" }, { token: "PRIVATE" }]) {
    assert.throws(() => client.enqueue(add(1), { ...preview(1), ...extra }), error("invalid_message"));
  }
  assert.throws(() => client.enqueue(add(1), { ...preview(1), source: "spotify" }), error("invalid_message"));
  client.disconnect(f.generation);
  for (let n = 1; n <= CLIENT_LIMITS.intents; n++) client.enqueue(add(n), preview(n));
  assert.throws(() => client.enqueue(add(99), preview(99)), error("pending_limit"));
  assert.equal(client.outbound(), null); assert.equal(ids(client).length, CLIENT_LIMITS.intents);
  assert.doesNotMatch(JSON.stringify(client.view()), /PRIVATE|local_path|url|token/);
  const shown = client.view(); shown.pending.length = 0; shown.intents[0].action.recording_id = "PRIVATE";
  assert.equal(ids(client).length, CLIENT_LIMITS.intents); assert.doesNotMatch(JSON.stringify(client.view()), /PRIVATE/);
});

test("malformed, conflicting and cross-room snapshots cannot change displayed state", async t => {
  const f = await fixture(t, 2), { client } = f;
  const before = client.view();
  for (const invalid of [
    { ...f.initial, room_id: uuid(999) }, { ...f.initial, revision: 0.5 },
    { ...f.initial, pending: Array(33).fill(preview(9)) },
    { ...f.initial, pending: [f.initial.current] },
    { ...f.initial, private_path: "PRIVATE" },
    { ...f.initial, pending: [preview(9)] },
  ]) {
    assert.throws(() => client.receiveSnapshot(f.generation, invalid), ClientError);
    assert.deepEqual(client.view(), before);
  }
});

test("an epoch change quarantines pending commands instead of replaying them in a new room", async t => {
  const f = await fixture(t), { client } = f;
  client.enqueue(add(1), preview(1)); client.outbound();
  assert.equal(client.receiveSnapshot(f.generation, { ...f.initial, epoch: uuid(99) }), false);
  assert.equal(client.view().blocked, "epoch_changed"); assert.deepEqual(ids(client), [occurrence(1)]);
  assert.equal(client.outbound(), null);
  const generation = client.beginConnection();
  assert.equal(client.receiveSnapshot(generation, f.initial), false);
  assert.equal(client.outbound(), null);
});

test("conflicting receipts never undo an accepted result waiting for its snapshot", async t => {
  const f = await fixture(t), { client } = f;
  client.enqueue(add(1), preview(1)); const packet = client.outbound();
  const accepted = await f.room.command(owner, packet);
  client.receiveReceipt(f.generation, accepted);
  assert.throws(() => client.receiveReceipt(f.generation, { ...accepted, accepted: false, error: "forbidden" }), error("receipt_conflict"));
  assert.equal(client.view().intents[0].state, "accepted");
  assert.throws(() => client.receiveReceipt(f.generation, { ...accepted, error: "PRIVATE" }), error("invalid_message"));
  await f.refresh(); assert.equal(client.view().intents.length, 0);
  assert.equal(client.receiveReceipt(f.generation, accepted), false);
});

test("temporary server failures require a snapshot and keep the original command intact", async t => {
  const f = await fixture(t), { client } = f;
  client.enqueue(add(1), preview(1)); const packet = client.outbound();
  client.receiveFailure(f.generation, packet.command_id, "state_unavailable");
  assert.equal(client.view().mode, "syncing"); assert.equal(client.outbound(), null);
  await f.refresh(); assert.deepEqual(client.outbound(), packet);
  client.receiveFailure(f.generation, packet.command_id, "room_expired");
  assert.equal(client.view().mode, "blocked"); assert.deepEqual(ids(client), [occurrence(1)]);
  assert.equal(client.outbound(), null);
});

test("member previews cannot grant owner controls or ownership of another person's rows", async t => {
  const f = await fixture(t, 2);
  const client = new RoomClient({ roomId, actorId: member }), generation = client.beginConnection();
  client.receiveSnapshot(generation, await f.room.snapshot(member));
  assert.throws(() => client.enqueue(advance), error("forbidden"));
  assert.throws(() => client.enqueue(remove(2)), error("forbidden"));
  assert.throws(() => client.enqueue(add(3), preview(3)), error("invalid_preview"));
  client.enqueue(add(3, 2), preview(3, member, 2)); assert.ok(client.outbound());
});

test("newer snapshots cannot replace known occurrence or recording identity", async t => {
  const f = await fixture(t, 2), { client } = f;
  const before = client.view();
  for (const change of [{ recording_id: recording(2) }, { added_by: member }, { duration_ms: 13000 }]) {
    const altered = structuredClone(f.initial);
    altered.revision++; altered.current = { ...altered.current, ...change };
    assert.throws(() => client.receiveSnapshot(f.generation, altered), error("snapshot_conflict"));
    assert.deepEqual(client.view(), before);
  }
  client.enqueue(add(3), preview(3));
  const pending = client.view(), altered = structuredClone(f.initial);
  altered.revision++; altered.pending.push(preview(3, member, 2));
  assert.throws(() => client.receiveSnapshot(f.generation, altered), error("snapshot_conflict"));
  assert.deepEqual(client.view(), pending);
});

test("media revisions cannot regress or disagree between repeated occurrences", async t => {
  const f = await fixture(t, 2), { client } = f;
  await f.room.publishMedia(recording(1), { owner_id: owner, revision: 2, state: "ready", duration_ms: 12000 });
  await f.refresh();
  const ready = await f.room.snapshot(owner), before = client.view();
  for (const change of [{ media_revision: 1, state: "uploading" }, { state: "unavailable" },
    { media_revision: 3, state: "unavailable" }]) {
    const altered = structuredClone(ready);
    altered.revision++; altered.pending[0] = { ...altered.pending[0], ...change };
    assert.throws(() => client.receiveSnapshot(f.generation, altered), error("snapshot_conflict"));
    assert.deepEqual(client.view(), before);
  }
  await f.room.publishMedia(recording(1), { owner_id: owner, revision: 3, state: "unavailable", duration_ms: 12000 });
  await f.refresh();
  assert.equal(client.view().current.state, "unavailable");
  assert.equal(client.view().pending[0].media_revision, 3);
});

test("returned command packets and caller-owned actions cannot mutate a retry", async t => {
  const f = await fixture(t), { client } = f;
  const action = add(1), selected = preview(1);
  client.enqueue(action, selected);
  action.occurrence_id = occurrence(9); selected.duration_ms = 13000;
  const packet = client.outbound(), expected = structuredClone(packet);
  packet.command_id++; packet.action.recording_id = recording(9);
  assert.deepEqual(client.outbound(), expected);
  assert.deepEqual(ids(client), [occurrence(1)]);
  assert.equal(client.view().pending[0].duration_ms, 12000);
});

test("room actor capacity blocks dispatch without hiding intent or allocating another ID", async t => {
  const f = await fixture(t), { client } = f;
  for (let n = 0; n < ROOM_LIMITS.actors; n++) {
    const actor = uuid(n + 10); f.members.add(actor);
    assert.equal((await f.send(actor, advance)).error, "forbidden");
  }
  client.enqueue(add(1), preview(1)); const packet = client.outbound();
  await assert.rejects(f.room.command(owner, packet), cause => cause.code === "actor_limit");
  client.receiveFailure(f.generation, packet.command_id, "actor_limit");
  assert.equal(client.view().blocked, "actor_limit"); assert.deepEqual(ids(client), [occurrence(1)]);
  const generation = client.beginConnection();
  client.receiveSnapshot(generation, await f.room.snapshot(owner));
  assert.equal(client.outbound(), null); assert.equal(client.view().intents[0].state, "sent");
});

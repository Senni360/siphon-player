import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { RoomQueue, RoomError, ROOM_LIMITS as L } from "../src/queue.mjs";
import { UploadLedger } from "../../media-gateway/src/uploads.mjs";

const uuid = n => `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
const owner = uuid(1), member = uuid(2), outsider = uuid(3), roomId = uuid(4), epoch = uuid(5);
const occurrence = n => uuid(n + 1000), recording = n => uuid(n + 2000);
const add = (n, track = 1) => ({ type: "add", occurrence_id: occurrence(n), recording_id: recording(track), explicit_selection: true });
const remove = n => ({ type: "remove", occurrence_id: occurrence(n) });
const packet = (id, revision, operation) => ({ epoch, command_id: id, expected_revision: revision, action: operation });
const error = code => value => value instanceof RoomError && value.code === code;

function fixture(t) {
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
  const media = new Map([
    [recording(1), { owner_id: owner, revision: 1, state: "uploading", duration_ms: 12000 }],
    [recording(2), { owner_id: member, revision: 1, state: "ready", duration_ms: 18000 }],
  ]);
  const options = { roomId, ownerId: owner, epoch, now: () => now,
    authorize: (actor, requestedRoom) => requestedRoom === roomId && members.has(actor),
    resolveRecording: (actor, id) => media.get(id)?.owner_id === actor ? { ...media.get(id) } : null };
  const f = { storage, options, members, media, advance: ms => { now += ms; }, now: () => now };
  f.restart = () => { f.queue = new RoomQueue(storage, options); };
  f.restart();
  f.send = async (actor, operation) => {
    const view = await f.queue.snapshot(actor);
    return f.queue.command(actor, packet(view.self.next_command_id, view.revision, operation));
  };
  return f;
}

test("explicit local selections append separate occurrences without exposing library metadata", async t => {
  const f = fixture(t);
  await f.send(owner, add(1)); await f.send(member, add(2, 2)); await f.send(owner, add(3));
  const view = await f.queue.snapshot(member);
  assert.equal(view.revision, 3); assert.equal(view.current, null);
  assert.deepEqual(view.pending.map(row => row.id), [occurrence(1), occurrence(2), occurrence(3)]);
  assert.deepEqual(view.pending.map(row => row.state), ["uploading", "ready", "uploading"]);
  assert.ok(view.pending.every(row => row.source === "shared_local"));
  assert.doesNotMatch(JSON.stringify(view), /sha256|token|url|local_path|filename|spotify|soundcloud/);
  assert.deepEqual(Object.keys(view.pending[0]).sort(), ["added_by", "duration_ms", "id", "media_revision", "recording_id", "source", "state"]);
});

test("membership is checked before snapshots and duplicate-command receipts", async t => {
  const f = fixture(t), command = packet(1, 0, add(1));
  const result = await f.queue.command(owner, command);
  await assert.rejects(f.queue.command(outsider, command), error("unavailable"));
  await assert.rejects(f.queue.snapshot(outsider), error("unavailable"));
  f.members.delete(owner);
  await assert.rejects(f.queue.command(owner, command), error("unavailable"));
  f.members.add(owner);
  assert.deepEqual(await f.queue.command(owner, command), result);
});

test("reopening a room cannot change its owner or epoch or reset its lifetime", async t => {
  const f = fixture(t); await f.send(owner, add(1));
  const original = await f.queue.snapshot(owner);
  for (const change of [{ ownerId: member }, { epoch: uuid(99) }, { roomId: uuid(98) }]) {
    assert.throws(() => new RoomQueue(f.storage, { ...f.options, ...change }), error("room_identity_conflict"));
  }
  f.advance(1000); f.restart();
  assert.deepEqual(await f.queue.snapshot(owner), original);
  f.advance(-1001);
  await assert.rejects(f.queue.snapshot(owner), error("invalid_clock"));
});

test("members remove only their pending occurrences; only the owner advances", async t => {
  const f = fixture(t);
  await f.send(owner, add(1)); await f.send(member, add(2, 2)); await f.send(member, add(3, 2));
  assert.equal((await f.send(member, remove(1))).error, "forbidden");
  assert.equal((await f.send(member, { type: "advance" })).error, "forbidden");
  assert.equal((await f.send(member, remove(2))).accepted, true);
  assert.deepEqual((await f.queue.snapshot(owner)).pending.map(row => row.id), [occurrence(1), occurrence(3)]);
  await f.send(owner, { type: "advance" });
  const view = await f.queue.snapshot(owner);
  assert.equal(view.current.id, occurrence(1)); assert.equal(view.current.state, "uploading");
  assert.equal((await f.send(owner, remove(1))).error, "not_pending");
  assert.equal((await f.send(owner, remove(3))).accepted, true);
  await f.send(owner, { type: "advance" });
  assert.equal((await f.queue.snapshot(owner)).current, null);
});

test("duplicate advance applies exactly once across restart and preserves its original revision", async t => {
  const f = fixture(t); await f.send(owner, add(1)); await f.send(owner, add(2));
  const command = packet(3, 2, { type: "advance" });
  const accepted = await f.queue.command(owner, command);
  f.restart(); await f.send(member, add(3, 2));
  assert.deepEqual(await f.queue.command(owner, command), accepted);
  const view = await f.queue.snapshot(owner);
  assert.equal(view.revision, 4); assert.equal(view.current.id, occurrence(1));
  assert.deepEqual(view.pending.map(row => row.id), [occurrence(2), occurrence(3)]);
  await assert.rejects(f.queue.command(owner, { ...command, action: remove(2) }), error("command_conflict"));
});

test("stale revisions are durable rejections, while stale epochs and command gaps never mutate", async t => {
  const f = fixture(t); await f.send(owner, add(1));
  const stale = packet(1, 0, add(2, 2));
  const rejected = await f.queue.command(member, stale);
  assert.deepEqual(rejected, { epoch, command_id: 1, revision: 1, accepted: false, error: "stale_revision" });
  await f.send(owner, add(3)); f.restart();
  assert.deepEqual(await f.queue.command(member, stale), rejected);
  await assert.rejects(f.queue.command(member, packet(3, 2, add(4, 2))), error("command_gap"));
  await assert.rejects(f.queue.command(member, { ...packet(2, 2, add(4, 2)), epoch: uuid(99) }), error("stale_epoch"));
  assert.equal((await f.send(member, add(4, 2))).accepted, true);
});

test("receipt eviction retains a high-water mark so an old skip can never run again", async t => {
  const f = fixture(t); await f.send(owner, add(1));
  const old = packet(2, 1, { type: "advance" }); await f.queue.command(owner, old);
  for (let n = 0; n < L.receipts + 2; n++) await f.send(owner, remove(999));
  assert.equal(f.queue.rows("SELECT COUNT(*) AS n FROM room_receipts")[0].n, L.receipts);
  f.restart();
  await assert.rejects(f.queue.command(owner, old), error("receipt_expired"));
  const view = await f.queue.snapshot(owner);
  assert.ok(view.self.receipt_floor > old.command_id);
  assert.equal(view.current.id, occurrence(1));
});

test("recording knowledge never grants ownership or readiness and extra private fields fail closed", async t => {
  const f = fixture(t);
  assert.equal((await f.send(member, add(1))).error, "recording_unavailable");
  for (const extra of [{ local_path: "PRIVATE" }, { url: "PRIVATE" }, { ready: true }, { provider_token: "PRIVATE" }]) {
    await assert.rejects(f.queue.command(owner, packet(1, 0, { ...add(1), ...extra })), error("invalid_fields"));
  }
  await assert.rejects(f.queue.command(owner, packet(1, 0, { ...add(1), explicit_selection: false })), error("invalid_action"));
  await assert.rejects(f.queue.command(owner, packet(1, 0, { ...add(1), recording_id: "spotify:track:x" })), error("invalid_action"));
  assert.equal((await f.queue.snapshot(owner)).revision, 0);
});

test("media events distinguish unavailable, uploading and ready without accepting old status", async t => {
  const f = fixture(t); await f.send(owner, add(1)); await f.send(owner, add(2));
  const update = { ...f.media.get(recording(1)), revision: 3, state: "ready" };
  assert.deepEqual(await f.queue.publishMedia(recording(1), update), { changed: true, revision: 3 });
  const old = { ...update, revision: 2, state: "unavailable" };
  assert.deepEqual(await f.queue.publishMedia(recording(1), old), { changed: false, revision: 3 });
  assert.equal((await f.queue.publishMedia(recording(1), update)).changed, false);
  await assert.rejects(f.queue.publishMedia(recording(1), { ...update, state: "uploading" }), error("media_revision_conflict"));
  await assert.rejects(f.queue.publishMedia(recording(1), { ...update, revision: 4, duration_ms: 50000 }), error("media_identity_conflict"));
  await f.send(owner, add(3)); // A stale resolver cannot downgrade known media.
  assert.ok((await f.queue.snapshot(owner)).pending.every(row => row.state === "ready"));
  await f.queue.publishMedia(recording(1), { ...update, revision: 4, state: "unavailable" });
  assert.ok((await f.queue.snapshot(owner)).pending.every(row => row.state === "unavailable"));
});

test("removed occurrences cannot be reused by another command or owner", async t => {
  const f = fixture(t); await f.send(owner, add(1)); await f.send(owner, remove(1));
  assert.equal((await f.send(owner, add(1))).error, "occurrence_used");
  assert.equal((await f.send(member, add(1, 2))).error, "occurrence_used");
});

test("queue, lifetime occurrence and actor limits bound durable room state", async t => {
  const f = fixture(t);
  for (let n = 0; n < L.pending; n++) assert.equal((await f.send(owner, add(n))).accepted, true);
  assert.equal((await f.send(owner, add(L.pending))).error, "queue_full");
  for (let n = 0; n < L.pending; n++) await f.send(owner, remove(n));
  for (let n = L.pending; n < L.occurrences; n++) { await f.send(owner, add(n)); await f.send(owner, remove(n)); }
  assert.equal((await f.send(owner, add(L.occurrences))).error, "occurrence_limit");
  for (let n = 0; n < L.actors - 1; n++) { const actor = uuid(500 + n); f.members.add(actor); await f.send(actor, { type: "advance" }); }
  f.members.add(outsider);
  await assert.rejects(f.send(outsider, { type: "advance" }), error("actor_limit"));
});

test("upload admission is limited to owned current-plus-two occurrences and removed rows remain cancellable", async t => {
  const f = fixture(t); for (let n = 1; n <= 4; n++) await f.send(owner, add(n));
  const uploads = new UploadLedger(f.storage, { now: f.now, authorize: (...args) => f.queue.allowsUpload(...args) });
  const input = n => ({ id: uuid(700 + n), room_id: roomId, occurrence_id: occurrence(n),
    size: 1, sha256: "a".repeat(64), profile: "mp3-44100-stereo", explicit_selection: true });
  await uploads.admit(owner, input(1)); await uploads.beginCreate(owner, input(1).id, 1);
  await uploads.settleCreate(input(1).id, 1, "multipart");
  await assert.rejects(uploads.admit(owner, input(4)), { code: "unavailable" });
  await assert.rejects(uploads.admit(member, input(2)), { code: "unavailable" });
  await f.send(owner, { type: "advance" }); await f.send(owner, { type: "advance" });
  assert.equal((await uploads.admit(owner, input(4))).state, "reserved");
  await assert.rejects(uploads.leasePart(owner, input(1).id, 1, 1), { code: "unavailable" });
  assert.equal((await uploads.cancel(owner, input(1).id, 1)).state, "cleanup");
  await f.send(owner, remove(4));
  assert.equal((await uploads.cancel(owner, input(4).id, 1)).state, "cancelled");
  assert.equal(f.queue.allowsUpload(owner, roomId, occurrence(3), "invented"), false);
});

test("expiry blocks new commands and uploads while retaining an owner's cancellation path", async t => {
  const f = fixture(t); await f.send(owner, add(1));
  f.advance(L.lifetimeMs - 1); assert.equal(f.queue.allowsUpload(owner, roomId, occurrence(1), "admit"), true);
  f.advance(1);
  await assert.rejects(f.queue.snapshot(owner), error("room_expired"));
  await assert.rejects(f.queue.command(owner, packet(2, 1, add(2))), error("room_expired"));
  assert.equal(f.queue.allowsUpload(owner, roomId, occurrence(1), "admit"), false);
  assert.equal(f.queue.allowsUpload(owner, roomId, occurrence(1), "cancel"), true);
  f.members.delete(owner);
  assert.equal(f.queue.allowsUpload(owner, roomId, occurrence(1), "cancel"), false);
});

test("acknowledgements wait for durable storage and lost acknowledgements do not duplicate additions", async t => {
  const f = fixture(t); const command = packet(1, 0, add(1));
  let release; const barrier = new Promise(resolve => { release = resolve; });
  f.storage.sync = () => barrier;
  let returned = false;
  const pending = f.queue.command(owner, command).then(result => { returned = true; return result; });
  await Promise.resolve(); assert.equal(returned, false);
  release(); const accepted = await pending;
  f.storage.sync = async () => { throw new Error("PRIVATE storage credentials"); };
  await assert.rejects(f.queue.command(owner, packet(2, 1, add(2))), error("state_unavailable"));
  f.storage.sync = async () => {}; f.restart();
  assert.deepEqual(await f.queue.command(owner, command), accepted);
  assert.equal((await f.queue.command(owner, packet(2, 1, add(2)))).accepted, true);
  assert.equal((await f.queue.snapshot(owner)).pending.length, 2);
});

test("concurrent commands against one revision cannot both change the queue", async t => {
  const f = fixture(t);
  const outcomes = await Promise.all([
    f.queue.command(owner, packet(1, 0, add(1))), f.queue.command(member, packet(1, 0, add(2, 2))),
  ]);
  assert.equal(outcomes.filter(result => result.accepted).length, 1);
  assert.equal(outcomes.find(result => !result.accepted).error, "stale_revision");
  assert.equal((await f.queue.snapshot(owner)).pending.length, 1);
});

test("a failed storage mutation rolls back the occurrence, media and command cursor together", async t => {
  const f = fixture(t), execute = f.storage.sql.exec;
  f.storage.sql.exec = (query, ...parameters) => {
    if (query.startsWith("UPDATE room_queue SET current_id")) throw new Error("PRIVATE database failure");
    return execute(query, ...parameters);
  };
  const command = packet(1, 0, add(1));
  await assert.rejects(f.queue.command(owner, command), error("state_unavailable"));
  f.storage.sql.exec = execute;
  const view = await f.queue.snapshot(owner);
  assert.equal(view.revision, 0); assert.equal(view.pending.length, 0);
  assert.equal(view.self.next_command_id, 1);
  assert.equal(f.queue.rows("SELECT COUNT(*) AS n FROM room_occurrences")[0].n, 0);
  assert.equal(f.queue.rows("SELECT COUNT(*) AS n FROM room_media")[0].n, 0);
  assert.equal((await f.queue.command(owner, command)).accepted, true);
});

test("authority callbacks fail closed and never expose private exception messages", async t => {
  const f = fixture(t);
  f.queue.authorize = async () => true;
  await assert.rejects(f.queue.snapshot(owner), error("unavailable"));
  f.queue.authorize = () => { throw new Error("PRIVATE provider credentials"); };
  await assert.rejects(f.queue.snapshot(owner), error("unavailable"));
  f.queue.authorize = f.options.authorize;
  f.queue.resolveRecording = () => { throw new Error("PRIVATE local library path"); };
  const rejected = await f.send(owner, add(1));
  assert.equal(rejected.error, "recording_unavailable"); assert.doesNotMatch(JSON.stringify(rejected), /PRIVATE/);
  f.queue.resolveRecording = async () => f.media.get(recording(1));
  assert.equal((await f.send(owner, add(1))).accepted, false);
});

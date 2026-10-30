import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { UploadLedger, UPLOAD_LIMITS as L } from "../src/uploads.mjs";
import { UploadCleanup, CleanupError } from "../src/upload-cleanup.mjs";

const uuid = number => `00000000-0000-4000-8000-${number.toString(16).padStart(12, "0")}`;
const owner = uuid(900), room = uuid(800);
const selection = (number = 1) => ({
  id: uuid(number), room_id: room, occurrence_id: uuid(number + 100),
  size: L.fileBytes, sha256: "a".repeat(64), profile: "mp3-44100-stereo", explicit_selection: true,
});
const key = number => `uploads/${uuid(number)}.mp3`;

class Bucket {
  uploads = new Set();
  objects = new Set();
  calls = [];
  hooks = {};
  resumeMultipartUpload(objectKey, id) {
    this.calls.push("resume");
    return { abort: async () => {
      this.calls.push("abort");
      await this.hooks.beforeAbort?.();
      if (!this.uploads.delete(id)) throw new Error("abort: Upload does not exist. (10024)");
      await this.hooks.afterAbort?.();
    } };
  }
  async delete(objectKey) {
    this.calls.push("delete");
    await this.hooks.beforeDelete?.();
    this.objects.delete(objectKey);
    await this.hooks.afterDelete?.();
  }
  async head(objectKey) {
    this.calls.push("head");
    await this.hooks.beforeHead?.();
    return this.objects.has(objectKey) ? { key: objectKey } : null;
  }
}

function fixture(t) {
  const db = new DatabaseSync(":memory:");
  t.after(() => db.close());
  let now = 1_800_000_000_000;
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
  const bucket = new Bucket();
  const f = { bucket, storage, advance: ms => { now += ms; } };
  f.restart = () => {
    f.ledger = new UploadLedger(storage, { now: () => now,
      authorize: (actor, requestedRoom) => actor === owner && requestedRoom === room });
    f.cleanup = new UploadCleanup(f.ledger, bucket);
  };
  f.restart();
  f.uploading = async (number = 1) => {
    const input = selection(number);
    await f.ledger.admit(owner, input);
    await f.ledger.beginCreate(owner, input.id, 1);
    bucket.uploads.add(`multipart-${number}`);
    return f.ledger.settleCreate(input.id, 1, `multipart-${number}`);
  };
  f.cancel = async (number = 1) => f.ledger.cancel(owner, uuid(number), 1);
  f.state = async (number = 1) => (await f.ledger.snapshot(owner, uuid(number))).state;
  return f;
}

test("abort, delete and confirmed absence precede quota release and leave a permanent tombstone", async t => {
  const f = fixture(t);
  for (let number = 1; number <= 4; number++) { await f.uploading(number); await f.cancel(number); }
  f.bucket.objects.add(key(1));
  await assert.rejects(f.ledger.admit(owner, selection(5)), { code: "quota_reached" });
  f.bucket.hooks.beforeHead = async () => {
    assert.equal(await f.state(), "cleanup");
    delete f.bucket.hooks.beforeHead;
  };
  assert.deepEqual(await f.cleanup.run(), { attempted: 4, cleaned: 4, retry: 0 });
  assert.deepEqual(f.bucket.calls.slice(0, 4), ["resume", "abort", "delete", "head"]);
  assert.equal(f.bucket.uploads.size, 0); assert.equal(f.bucket.objects.size, 0);
  f.restart();
  assert.equal(await f.state(), "cancelled");
  assert.equal((await f.ledger.admit(owner, selection())).state, "cancelled");
  assert.equal((await f.ledger.admit(owner, selection(5))).state, "reserved");
  assert.deepEqual(await f.cleanup.run(), { attempted: 0, cleaned: 0, retry: 0 });
});

test("unknown creation or completion is never mistaken for storage absence", async t => {
  const f = fixture(t);
  await f.ledger.admit(owner, selection()); await f.ledger.beginCreate(owner, uuid(1), 1);
  await f.cancel();
  await f.uploading(2);
  for (const number of [1, 2]) {
    const lease = await f.ledger.leasePart(owner, uuid(2), 1, number);
    await f.ledger.recordPart(uuid(2), 1, { number, lease_id: lease.lease_id, size: lease.size, etag: `part-${number}` });
  }
  await f.ledger.beginComplete(owner, uuid(2), 1); await f.cancel(2);
  f.restart();
  assert.deepEqual(await f.cleanup.run(), { attempted: 0, cleaned: 0, retry: 0 });
  assert.deepEqual(f.bucket.calls, []);
  assert.equal(await f.state(), "cleanup"); assert.equal(await f.state(2), "cleanup");
  // Creation eventually returns its real handle. Only that known task proceeds.
  f.bucket.uploads.add("late"); await f.ledger.settleCreate(uuid(1), 1, "late");
  assert.deepEqual(await f.cleanup.run(), { attempted: 1, cleaned: 1, retry: 0 });
  assert.equal(await f.state(2), "cleanup");
});

for (const point of ["beforeAbort", "afterAbort", "beforeDelete", "afterDelete", "beforeHead"]) {
  test(`failure at ${point} retains quota and retries safely after restart`, async t => {
    const f = fixture(t); await f.uploading(); await f.cancel();
    f.bucket.objects.add(key(1));
    f.bucket.hooks[point] = () => { throw new Error("PRIVATE provider credentials and handles"); };
    assert.deepEqual(await f.cleanup.run(), { attempted: 1, cleaned: 0, retry: 1 });
    assert.equal(await f.state(), "cleanup");
    f.restart();
    assert.deepEqual(await f.cleanup.run(), { attempted: 0, cleaned: 0, retry: 0 });
    delete f.bucket.hooks[point]; f.advance(L.retryMs);
    assert.deepEqual(await f.cleanup.run(), { attempted: 1, cleaned: 1, retry: 0 });
    assert.equal(await f.state(), "cancelled");
    assert.equal(f.bucket.uploads.size, 0); assert.equal(f.bucket.objects.size, 0);
  });
}

test("a successful delete without confirmed absence never releases quota", async t => {
  const f = fixture(t); await f.uploading(); await f.cancel();
  f.bucket.hooks.beforeHead = () => { f.bucket.objects.add(key(1)); };
  assert.deepEqual(await f.cleanup.run(), { attempted: 1, cleaned: 0, retry: 1 });
  assert.equal(await f.state(), "cleanup");
  delete f.bucket.hooks.beforeHead; f.advance(L.retryMs);
  assert.equal((await f.cleanup.run()).cleaned, 1);
});

test("only R2's precise NoSuchUpload code counts as a missing multipart upload", async t => {
  for (const message of ["404 Not Found", "NoSuchUpload", "denied (10003)", "(10024) timeout", "timeout (10001)"]) {
    const f = fixture(t); await f.uploading(); await f.cancel();
    f.bucket.hooks.beforeAbort = () => { throw new Error(message); };
    assert.equal((await f.cleanup.run()).retry, 1);
    assert.equal(await f.state(), "cleanup");
    assert.deepEqual(f.bucket.calls, ["resume", "abort"]);
  }
});

test("a confirmed never-created upload still checks its object key before acknowledgement", async t => {
  const f = fixture(t);
  await f.ledger.admit(owner, selection()); await f.ledger.beginCreate(owner, uuid(1), 1);
  await f.ledger.settleCreate(uuid(1), 1, null);
  assert.equal((await f.cleanup.run()).cleaned, 1);
  assert.deepEqual(f.bucket.calls, ["delete", "head"]);
});

test("overlapping sweeps share bounded work and wait for durable acknowledgement", async t => {
  const f = fixture(t); await f.uploading(); await f.cancel();
  let release;
  const blocked = new Promise(resolve => { release = resolve; });
  f.bucket.hooks.beforeAbort = () => blocked;
  const first = f.cleanup.run(), second = f.cleanup.run();
  assert.equal(first, second);
  let finishSync;
  const durable = new Promise(resolve => { finishSync = resolve; });
  let reachedHead;
  const headEntered = new Promise(resolve => { reachedHead = resolve; });
  f.bucket.hooks.beforeHead = () => { f.storage.sync = () => durable; reachedHead(); };
  let returned = false;
  first.then(() => { returned = true; });
  release();
  // Wait for the controlled acknowledgement without using wall-clock delays.
  await headEntered;
  await Promise.resolve(); assert.equal(returned, false);
  finishSync();
  assert.deepEqual(await first, { attempted: 1, cleaned: 1, retry: 0 });
  assert.equal(f.bucket.calls.filter(call => call === "abort").length, 1);
});

test("persistence failures are private and successful physical cleanup can be acknowledged again", async t => {
  const f = fixture(t); await f.uploading(); await f.cancel();
  f.bucket.hooks.beforeHead = () => {
    f.storage.sync = async () => { throw new Error("PRIVATE sql/handle/token"); };
  };
  await assert.rejects(f.cleanup.run(), error => error instanceof CleanupError && !error.message.includes("PRIVATE"));
  f.storage.sync = async () => {}; delete f.bucket.hooks.beforeHead;
  f.restart();
  // Simulated storage kept the transaction despite a failed durability reply.
  assert.equal(await f.state(), "cancelled");
  assert.equal((await f.cleanup.run()).attempted, 0);
});

test("invalid task keys or an oversized batch cannot cause storage operations", async () => {
  const valid = { id: uuid(1), generation: 2, key: key(1), multipart_id: "multipart" };
  for (const tasks of [
    [{ ...valid, key: "other-owner/private.mp3" }], [{ ...valid, id: [uuid(1)] }],
    [{ ...valid, generation: 0 }], [{ ...valid, multipart_id: "bad\nhandle" }],
    Array(17).fill(valid),
  ]) {
    const bucket = new Bucket();
    const cleanup = new UploadCleanup({ cleanupTasks: async () => tasks }, bucket);
    await assert.rejects(cleanup.run(), CleanupError);
    assert.deepEqual(bucket.calls, []);
  }
});

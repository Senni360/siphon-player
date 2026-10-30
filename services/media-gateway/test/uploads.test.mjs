import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { UploadLedger, UploadError, UPLOAD_LIMITS as L } from "../src/uploads.mjs";

const uuid = number => `00000000-0000-4000-8000-${number.toString(16).padStart(12, "0")}`;
const alice = uuid(900), bob = uuid(901), carol = uuid(902), room = uuid(800);
const input = (number = 1, size = L.partBytes + 37) => ({
  id: uuid(number), room_id: room, occurrence_id: uuid(number + 100), size,
  sha256: "a".repeat(64), profile: "mp3-44100-stereo", explicit_selection: true,
});
const error = code => value => value instanceof UploadError && value.code === code;

function fixture(t) {
  const db = new DatabaseSync(":memory:");
  t.after(() => db.close());
  let time = 1_800_000_000_000;
  const members = new Set([alice, bob, carol]);
  const eligible = new Set(Array.from({ length: 100 }, (_, index) => uuid(index + 100)));
  const storage = {
    syncs: 0,
    sql: { exec(query, ...parameters) {
      const statement = db.prepare(query);
      const rows = statement.columns().length ? statement.all(...parameters) : (statement.run(...parameters), []);
      return { toArray: () => rows.map(row => ({ ...row })) };
    } },
    transactionSync(operation) {
      db.exec("BEGIN");
      try { const value = operation(); db.exec("COMMIT"); return value; }
      catch (failure) { db.exec("ROLLBACK"); throw failure; }
    },
    async sync() { this.syncs++; },
  };
  const options = { now: () => time, authorize: (actor, requestedRoom, occurrence, operation) =>
    members.has(actor) && requestedRoom === room
      && (!["admit", "upload"].includes(operation) || eligible.has(occurrence)) };
  const f = { storage, options, members, eligible, now: () => time, advance: ms => { time += ms; } };
  f.ledger = new UploadLedger(storage, options);
  f.restart = () => { f.ledger = new UploadLedger(storage, options); };
  f.uploading = async (selection = input(), actor = alice) => {
    await f.ledger.admit(actor, selection);
    const plan = await f.ledger.beginCreate(actor, selection.id, 1);
    assert.equal(plan.key, `uploads/${selection.id}.mp3`);
    return f.ledger.settleCreate(selection.id, 1, `multipart-${selection.id}`);
  };
  f.parts = async (selection = input(), actor = alice) => {
    const snapshot = await f.ledger.snapshot(actor, selection.id);
    for (const part of snapshot.parts) {
      const lease = await f.ledger.leasePart(actor, selection.id, 1, part.number);
      await f.ledger.recordPart(selection.id, 1, {
        number: lease.number, lease_id: lease.lease_id, size: lease.size, etag: `part-${lease.number}`,
      });
    }
  };
  f.proof = (selection = input()) => ({ size: selection.size, sha256: selection.sha256,
    profile: selection.profile, etag: "object-etag", version: "object-version" });
  f.ready = async (selection = input(), actor = alice) => {
    await f.uploading(selection, actor); await f.parts(selection, actor);
    await f.ledger.beginComplete(actor, selection.id, 1);
    return f.ledger.settleComplete(selection.id, 1, f.proof(selection));
  };
  return f;
}

test("admission needs explicit selection, trusted membership and current queue eligibility", async t => {
  const f = fixture(t);
  assert.throws(() => new UploadLedger(f.storage), error("authority_required"));
  for (const change of [{ explicit_selection: false }, { profile: "wav" }, { size: L.fileBytes + 1 }, { id: [uuid(1)] }]) {
    await assert.rejects(f.ledger.admit(alice, { ...input(), ...change }), error("invalid_selection"));
  }
  for (const change of [{ local_path: "PRIVATE/library/song.mp3" }, { provider_token: "PRIVATE" }, { filename: "PRIVATE.mp3" }]) {
    await assert.rejects(f.ledger.admit(alice, { ...input(), ...change }), error("invalid_fields"));
  }
  f.eligible.delete(input().occurrence_id);
  await assert.rejects(f.ledger.admit(alice, input()), error("unavailable"));
  assert.equal(f.ledger.rows("SELECT COUNT(*) AS count FROM upload_sessions")[0].count, 0);
  const wrongAuthority = new UploadLedger(f.storage, { authorize: async () => true });
  await assert.rejects(wrongAuthority.admit(alice, input()), error("unavailable"));
});

test("duplicate admissions preserve reservation and deadline, and file changes cannot reuse an identity", async t => {
  const f = fixture(t);
  const first = await f.ledger.admit(alice, input());
  f.advance(1000); f.restart();
  assert.deepEqual(await f.ledger.admit(alice, input()), first);
  assert.deepEqual(await f.ledger.snapshot(alice, input().id, { size: input().size, sha256: input().sha256 }), first);
  await assert.rejects(f.ledger.admit(alice, { ...input(), sha256: "b".repeat(64) }), error("identity_conflict"));
  await assert.rejects(f.ledger.snapshot(alice, input().id, { size: input().size, sha256: "b".repeat(64) }), error("file_changed"));
  await assert.rejects(f.ledger.admit(alice, { ...input(2), occurrence_id: input().occurrence_id }), error("occurrence_already_reserved"));
  assert.equal(first.state, "reserved"); assert.equal(first.uploaded_bytes, 0);
});

test("a known recording hash never grants another owner's reservation or bytes", async t => {
  const f = fixture(t);
  await f.ready();
  await assert.rejects(f.ledger.snapshot(bob, input().id), error("unavailable"));
  await assert.rejects(f.ledger.cancel(bob, input().id, 1), error("unavailable"));
  await assert.rejects(f.ledger.admit(bob, input()), error("unavailable"));
  const second = await f.ledger.admit(bob, input(2));
  assert.equal(second.state, "reserved"); assert.notEqual(second.id, input().id);
  f.members.delete(alice);
  await assert.rejects(f.ledger.admit(alice, input()), error("unavailable"));
  await assert.rejects(f.ledger.snapshot(alice, input().id), error("unavailable"));
});

test("lookahead admits at most three active selections and leaving the window stops new leases", async t => {
  const f = fixture(t);
  for (let index = 1; index <= 3; index++) await f.ledger.admit(alice, input(index));
  await assert.rejects(f.ledger.admit(alice, input(4)), error("quota_reached"));
  await f.ledger.cancel(alice, input(3).id, 1);
  await f.ledger.admit(alice, input(4));
  await f.ledger.beginCreate(alice, input().id, 1);
  await f.ledger.settleCreate(input().id, 1, "multipart");
  f.eligible.delete(input().occurrence_id);
  await assert.rejects(f.ledger.leasePart(alice, input().id, 1, 1), error("unavailable"));
  assert.equal((await f.ledger.cancel(alice, input().id, 1)).state, "cleanup");
});

test("owner and total storage quotas include ready files and cancelled bytes awaiting cleanup", async t => {
  const f = fixture(t);
  for (let index = 1; index <= 4; index++) await f.ready(input(index, L.fileBytes));
  await assert.rejects(f.ledger.admit(alice, input(5, 1)), error("quota_reached"));
  await f.ledger.cancel(alice, input().id, 1);
  await assert.rejects(f.ledger.admit(alice, input(5, 1)), error("quota_reached"));
  for (let index = 6; index <= 9; index++) await f.ready(input(index, L.fileBytes), bob);
  await assert.rejects(f.ledger.admit(carol, input(10, 1)), error("quota_reached"));
  await f.ledger.acknowledgeCleanup(input().id, 2);
  assert.equal((await f.ledger.admit(carol, input(10, L.fileBytes))).state, "reserved");
});

test("cancellation before admission reserves a bounded tombstone and defeats delayed retries", async t => {
  const f = fixture(t);
  const cancelled = await f.ledger.cancelBeforeAdmission(alice, input());
  assert.equal(cancelled.state, "cancelled"); assert.equal(cancelled.generation, 2);
  f.restart();
  assert.deepEqual(await f.ledger.admit(alice, input()), cancelled);
  await assert.rejects(f.ledger.beginCreate(alice, input().id, 1), error("stale_generation"));
  await assert.rejects(f.ledger.cancelBeforeAdmission(bob, input()), error("unavailable"));
  for (let index = 2; index <= L.identities; index++) await f.ledger.cancelBeforeAdmission(alice, input(index));
  await assert.rejects(f.ledger.admit(alice, input(L.identities + 1)), error("quota_reached"));
});

test("creation and completion effects are claimed once and never returned before durable storage", async t => {
  const f = fixture(t);
  await f.ledger.admit(alice, input());
  let release;
  const synced = new Promise(resolve => { release = resolve; });
  f.storage.sync = () => synced;
  let returned = false;
  const pending = f.ledger.beginCreate(alice, input().id, 1).then(value => { returned = true; return value; });
  await Promise.resolve(); assert.equal(returned, false);
  release(); assert.equal((await pending).generation, 1);
  f.restart();
  assert.equal(await f.ledger.beginCreate(alice, input().id, 1), null);
  await f.ledger.settleCreate(input().id, 1, "multipart");
  assert.equal(await f.ledger.beginCreate(alice, input().id, 1), null);
  await f.parts();
  assert.equal((await f.ledger.beginComplete(alice, input().id, 1)).parts.length, 2);
  assert.equal(await f.ledger.beginComplete(alice, input().id, 1), null);
});

test("cancellation during creation keeps quota until the late handle is aborted and deleted", async t => {
  const f = fixture(t);
  await f.ledger.admit(alice, input()); await f.ledger.beginCreate(alice, input().id, 1);
  const cancelled = await f.ledger.cancel(alice, input().id, 1);
  assert.equal(cancelled.state, "cleanup");
  assert.deepEqual(await f.ledger.cancel(alice, input().id, 1), cancelled);
  f.restart(); assert.deepEqual(await f.ledger.cleanupTasks(), []);
  await assert.rejects(f.ledger.acknowledgeCleanup(input().id, 2), error("storage_outcome_pending"));
  assert.equal((await f.ledger.settleCreate(input().id, 1, "late-multipart")).state, "cleanup");
  assert.equal((await f.ledger.cleanupTasks())[0].multipart_id, "late-multipart");
  await assert.rejects(f.ledger.leasePart(alice, input().id, 1, 1), error("stale_generation"));
  await f.ledger.acknowledgeCleanup(input().id, 2);
  assert.equal((await f.ledger.settleCreate(input().id, 1, "late-multipart")).state, "cancelled");
  await assert.rejects(f.ledger.settleCreate(input().id, 1, "different-multipart"), error("storage_result_conflict"));
});

test("part leases are bounded across an owner's uploads and survive restart and duplicate requests", async t => {
  const f = fixture(t);
  await f.uploading(); await f.uploading(input(2));
  const first = await f.ledger.leasePart(alice, input().id, 1, 1);
  const second = await f.ledger.leasePart(alice, input().id, 1, 2);
  assert.equal(first.size, L.partBytes); assert.equal(second.offset, L.partBytes); assert.equal(second.size, 37);
  f.restart();
  assert.deepEqual(await f.ledger.leasePart(alice, input().id, 1, 1), { ...first, revision: second.revision });
  await assert.rejects(f.ledger.leasePart(alice, input(2).id, 1, 1), error("part_limit"));
  await f.ledger.cancel(alice, input().id, 1);
  // In-flight parts still consume capacity while their abort is pending.
  await assert.rejects(f.ledger.leasePart(alice, input(2).id, 1, 1), error("part_limit"));
  await f.ledger.acknowledgeCleanup(input().id, 2);
  assert.equal((await f.ledger.leasePart(alice, input(2).id, 1, 1)).number, 1);
});

test("lease expiry renews only that part and rejects stale receipts without losing accepted progress", async t => {
  const f = fixture(t); await f.uploading();
  const old = await f.ledger.leasePart(alice, input().id, 1, 1);
  f.advance(L.leaseMs);
  const renewed = await f.ledger.leasePart(alice, input().id, 1, 1);
  assert.notEqual(renewed.lease_id, old.lease_id);
  await assert.rejects(f.ledger.recordPart(input().id, 1, { number: 1, lease_id: old.lease_id, size: old.size, etag: "old" }), error("stale_part"));
  const receipt = { number: 1, lease_id: renewed.lease_id, size: renewed.size, etag: "accepted" };
  const accepted = await f.ledger.recordPart(input().id, 1, receipt);
  assert.equal(accepted.uploaded_bytes, L.partBytes); assert.ok(accepted.revision > renewed.revision);
  f.restart(); assert.deepEqual(await f.ledger.recordPart(input().id, 1, receipt), accepted);
  await assert.rejects(f.ledger.recordPart(input().id, 1, { ...receipt, etag: "changed" }), error("storage_result_conflict"));
  await assert.rejects(f.ledger.beginComplete(alice, input().id, 1), error("parts_missing"));
  await f.ledger.cancel(alice, input().id, 1);
  await assert.rejects(f.ledger.recordPart(input().id, 1, receipt), error("stale_generation"));
});

test("ready requires verified bytes and duplicate completion preserves the original result", async t => {
  const f = fixture(t); const ready = await f.ready();
  assert.equal(ready.state, "ready"); assert.equal(ready.uploaded_bytes, input().size);
  assert.equal(await f.ledger.beginCreate(alice, input().id, 1), null);
  assert.equal(await f.ledger.beginComplete(alice, input().id, 1), null);
  const part = f.ledger.rows("SELECT * FROM upload_parts WHERE upload_id=? AND number=1", input().id)[0];
  assert.deepEqual(await f.ledger.recordPart(input().id, 1, {
    number: 1, lease_id: part.lease_id, size: part.size, etag: part.etag,
  }), ready);
  assert.deepEqual(await f.ledger.settleComplete(input().id, 1, f.proof()), ready);
  await assert.rejects(f.ledger.settleComplete(input().id, 1, { ...f.proof(), version: "replacement" }), error("storage_result_conflict"));
  for (const [index, mismatch] of [[2, { sha256: "b".repeat(64) }], [3, { size: L.fileBytes + 1 }], [4, { profile: "wav" }]]) {
    const selection = input(index); await f.uploading(selection); await f.parts(selection);
    await f.ledger.beginComplete(alice, selection.id, 1);
    const rejected = await f.ledger.settleComplete(selection.id, 1, { ...f.proof(selection), ...mismatch });
    assert.equal(rejected.state, "cleanup"); assert.equal(rejected.reason, "verification_failed");
  }
});

test("opaque Workers part receipts survive unchanged while oversized or malformed tokens fail closed", async t => {
  const f = fixture(t); await f.uploading();
  const first = await f.ledger.leasePart(alice, input().id, 1, 1);
  for (const etag of ["", "x".repeat(1025), '"quoted"', "bad\r\nvalue", "non-ascii-\u00e9"]) {
    await assert.rejects(f.ledger.recordPart(input().id, 1, {
      number: 1, lease_id: first.lease_id, size: first.size, etag,
    }), error("invalid_storage_result"));
  }
  assert.equal((await f.ledger.snapshot(alice, input().id)).uploaded_bytes, 0);
  const tokens = ["r".repeat(171), "s".repeat(1024)];
  for (const [index, etag] of tokens.entries()) {
    const lease = await f.ledger.leasePart(alice, input().id, 1, index + 1);
    await f.ledger.recordPart(input().id, 1, {
      number: lease.number, lease_id: lease.lease_id, size: lease.size, etag,
    });
  }
  f.restart();
  const effect = await f.ledger.beginComplete(alice, input().id, 1);
  assert.deepEqual(effect.parts.map(part => part.etag), tokens);
  assert.ok(!JSON.stringify(await f.ledger.snapshot(alice, input().id)).includes(tokens[0]));
});

test("late completion cannot resurrect cancellation and cleanup waits for an uncertain outcome", async t => {
  const f = fixture(t); await f.uploading(); await f.parts();
  await f.ledger.beginComplete(alice, input().id, 1);
  await f.ledger.cancel(alice, input().id, 1);
  f.restart(); assert.deepEqual(await f.ledger.cleanupTasks(), []);
  await assert.rejects(f.ledger.acknowledgeCleanup(input().id, 2), error("storage_outcome_pending"));
  const late = await f.ledger.settleComplete(input().id, 1, f.proof());
  assert.equal(late.state, "cleanup"); assert.equal(late.generation, 2);
  assert.equal((await f.ledger.cleanupTasks()).length, 1);
  await f.ledger.acknowledgeCleanup(input().id, 2);
  await f.ledger.acknowledgeCleanup(input().id, 2);
  assert.equal((await f.ledger.settleComplete(input().id, 1, f.proof())).state, "cancelled");
  assert.equal((await f.ledger.admit(alice, input())).state, "cancelled");
});

test("expiry is absolute and cleanup retries survive restart with bounded backoff", async t => {
  const f = fixture(t); const ready = await f.ready();
  f.advance(L.retentionMs - 1); assert.equal((await f.ledger.snapshot(alice, input().id)).state, "ready");
  f.advance(1);
  const task = (await f.ledger.cleanupTasks())[0]; assert.equal(task.generation, 2);
  const expired = await f.ledger.snapshot(alice, input().id);
  assert.equal(expired.state, "cleanup"); assert.equal(expired.reason, "expired");
  assert.equal(expired.expires_at, ready.expires_at);
  for (let attempt = 0; attempt < 7; attempt++) {
    await f.ledger.cleanupFailed(task.id, task.generation); f.restart();
    assert.deepEqual(await f.ledger.cleanupTasks(), []);
    const delay = Math.min(L.retryMs * 2 ** attempt, L.maxRetryMs);
    f.advance(delay - 1); assert.deepEqual(await f.ledger.cleanupTasks(), []);
    f.advance(1); assert.equal((await f.ledger.cleanupTasks()).length, 1);
  }
  await f.ledger.acknowledgeCleanup(task.id, task.generation);
  assert.deepEqual(await f.ledger.cleanupTasks(), []);
});

test("public progress contains no storage handles, file hashes, credentials or arbitrary metadata", async t => {
  const f = fixture(t); await f.uploading();
  const snapshot = JSON.stringify(await f.ledger.snapshot(alice, input().id));
  assert.doesNotMatch(snapshot, /multipart|sha256|uploads\/|token|local_path|filename/);
  assert.ok(f.storage.syncs >= 4);
});

test("storage-confirmed oversized parts trigger cleanup rather than accepting progress", async t => {
  const f = fixture(t); await f.uploading();
  const lease = await f.ledger.leasePart(alice, input().id, 1, 1);
  const rejected = await f.ledger.recordPart(input().id, 1, {
    number: 1, lease_id: lease.lease_id, size: L.partBytes + 1, etag: "oversized",
  });
  assert.equal(rejected.state, "cleanup"); assert.equal(rejected.reason, "part_verification_failed");
  assert.equal(rejected.uploaded_bytes, 0);
  await assert.rejects(f.ledger.leasePart(alice, input().id, 1, 2), error("stale_generation"));
});

test("observed storage overages remain charged until cleanup instead of hiding behind the declared size", async t => {
  const f = fixture(t); await f.uploading();
  const lease = await f.ledger.leasePart(alice, input().id, 1, 1);
  await f.ledger.recordPart(input().id, 1, {
    number: 1, lease_id: lease.lease_id, size: L.ownerBytes + 1, etag: "oversized",
  });
  await assert.rejects(f.ledger.admit(alice, input(2, 1)), error("quota_reached"));
  await f.ledger.acknowledgeCleanup(input().id, 2);
  await f.uploading(input(2)); await f.parts(input(2));
  await f.ledger.beginComplete(alice, input(2).id, 1);
  await f.ledger.settleComplete(input(2).id, 1, { ...f.proof(input(2)), size: L.totalBytes + 1 });
  await assert.rejects(f.ledger.admit(bob, input(3, 1)), error("quota_reached"));
  await f.ledger.acknowledgeCleanup(input(2).id, 2);
  assert.equal((await f.ledger.admit(bob, input(3, 1))).state, "reserved");
});

test("overlapping admissions cannot overbook the last active slot during persistence", async t => {
  const f = fixture(t);
  let release;
  const synced = new Promise(resolve => { release = resolve; });
  f.storage.sync = () => synced;
  const admissions = Promise.allSettled([1, 2, 3, 4].map(index => f.ledger.admit(alice, input(index))));
  release();
  const results = await admissions;
  assert.equal(results.filter(result => result.status === "fulfilled").length, 3);
  const rejected = results.find(result => result.status === "rejected");
  assert.equal(rejected.reason.code, "quota_reached");
});

test("a failed durability acknowledgement never returns or repeats a provider effect", async t => {
  const f = fixture(t); await f.ledger.admit(alice, input());
  const original = f.storage.sync;
  f.storage.sync = async () => { throw new Error("disk unavailable"); };
  await assert.rejects(f.ledger.beginCreate(alice, input().id, 1), /disk unavailable/);
  f.storage.sync = original; f.restart();
  assert.equal(await f.ledger.beginCreate(alice, input().id, 1), null);
  await f.ledger.cancel(alice, input().id, 1);
  await assert.rejects(f.ledger.acknowledgeCleanup(input().id, 2), error("storage_outcome_pending"));
  // An independent provider reconciliation can now confirm no upload exists.
  await f.ledger.settleCreate(input().id, 1, null);
  await f.ledger.acknowledgeCleanup(input().id, 2);
  assert.equal((await f.ledger.snapshot(alice, input().id)).state, "cancelled");
});

test("expiry during an outstanding completion never makes the recording ready", async t => {
  const f = fixture(t); await f.uploading(); await f.parts();
  await f.ledger.beginComplete(alice, input().id, 1);
  f.advance(L.retentionMs); f.restart();
  const result = await f.ledger.settleComplete(input().id, 1, f.proof());
  assert.equal(result.state, "cleanup"); assert.equal(result.reason, "expired");
  assert.equal(result.generation, 2); assert.equal((await f.ledger.cleanupTasks()).length, 1);
});

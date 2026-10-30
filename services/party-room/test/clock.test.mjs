import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { RoomClock, ClockError, CLOCK_LIMITS as L } from "../src/clock.mjs";
import { RoomQueue, RoomError, ROOM_LIMITS } from "../src/queue.mjs";
import { projectTimeline } from "../src/timeline.mjs";

const uuid = n => `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
const owner = uuid(1), member = uuid(2), roomId = uuid(3), epoch = uuid(4);
const OFFSET = 1_800_000_000_000;
const error = code => cause => cause instanceof ClockError && cause.code === code;
const roomError = code => cause => cause instanceof RoomError && cause.code === code;

function fixture({ driftPpm = 0 } = {}) {
  let mono = 1000, offset = OFFSET;
  const f = { now: () => mono, advance: ms => { mono += ms; }, shiftServer: ms => { offset += ms; },
    server: () => offset + mono * (1 + driftPpm / 1_000_000) };
  f.clock = new RoomClock({ roomId, now: () => mono });
  f.generation = f.clock.beginConnection(epoch);
  f.reply = (probe, received = Math.round(f.server()), sent = received) => ({ room_id: roomId, epoch,
    probe_id: probe.probe_id, received_at: received, sent_at: sent });
  f.sample = ({ outgoing = 20, incoming = 20, processing = 0 } = {}) => {
    const probe = f.clock.probe(f.generation); f.advance(outgoing);
    const received = Math.round(f.server()); f.advance(processing);
    const sent = Math.round(f.server()); f.advance(incoming);
    return f.clock.accept(f.generation, f.reply(probe, received, sent));
  };
  f.prime = options => { for (let n = 0; n < L.minimumSamples; n++) { assert.equal(f.sample(options), true); f.advance(100); } };
  f.estimate = () => f.clock.estimate(f.generation);
  return f;
}

function withinBound(f) {
  const result = f.estimate(); assert.equal(result.status, "ready");
  assert.ok(Math.abs(result.server_time_ms - f.server()) <= result.uncertainty_ms,
    "synthetic server time lies within the reported uncertainty");
  return result;
}

test("an estimate needs several distinct samples and advances only from monotonic time", () => {
  const f = fixture(); assert.deepEqual(f.estimate(), { status: "unavailable", reason: "collecting" });
  f.sample(); f.sample(); assert.equal(f.estimate().status, "unavailable");
  f.sample(); const first = withinBound(f);
  assert.equal(first.sample_count, 3); assert.equal(first.minimum_rtt_ms, 40);
  f.advance(1234); const second = withinBound(f);
  assert.equal(second.server_time_ms - first.server_time_ms, 1234);
});

test("server processing time is removed from the path-delay bound", () => {
  const f = fixture(); f.prime({ outgoing: 10, incoming: 10, processing: 700 });
  const result = withinBound(f); assert.equal(result.minimum_rtt_ms, 20);
  assert.ok(result.uncertainty_ms < 20);
});

test("asymmetric paths stay inside uncertainty rather than claiming midpoint accuracy", () => {
  for (const [outgoing, incoming] of [[0, 120], [120, 0], [20, 90], [90, 20]]) {
    const f = fixture(); f.prime({ outgoing, incoming }); const result = withinBound(f);
    assert.ok(result.uncertainty_ms >= Math.abs(outgoing - incoming) / 2);
  }
});

test("low-delay samples tighten the estimate while slow samples cannot pull it away", () => {
  const f = fixture(); f.prime({ outgoing: 400, incoming: 400 });
  assert.deepEqual(f.estimate(), { status: "unavailable", reason: "uncertainty" });
  f.sample({ outgoing: 10, incoming: 10 }); const tight = withinBound(f);
  assert.ok(tight.uncertainty_ms < 20);
  f.sample({ outgoing: 500, incoming: 100 }); const after = withinBound(f);
  assert.equal(after.minimum_rtt_ms, 20); assert.ok(after.uncertainty_ms < 20);
});

test("deterministic delay and relative-drift scenarios remain inside the declared bounds", () => {
  for (const driftPpm of [-L.relativeDriftPpm, 0, L.relativeDriftPpm]) {
    for (const [outgoing, incoming, processing] of [[1, 1, 0], [10, 80, 50], [80, 10, 400], [50, 50, 100]]) {
      const f = fixture({ driftPpm }); f.prime({ outgoing, incoming, processing });
      for (let n = 0; n < 20; n++) { f.advance(1000); withinBound(f); }
    }
  }
});

test("duplicate, unknown and old-connection replies cannot contribute samples", () => {
  const f = fixture(), probe = f.clock.probe(f.generation), reply = f.reply(probe);
  assert.equal(f.clock.accept(f.generation, reply), true);
  assert.equal(f.clock.accept(f.generation, reply), false);
  assert.equal(f.clock.accept(f.generation, { ...reply, probe_id: 9999 }), false);
  assert.equal(f.estimate().status, "unavailable");
  const oldGeneration = f.generation; f.generation = f.clock.beginConnection(epoch);
  assert.equal(f.clock.accept(oldGeneration, reply), false);
  assert.equal(f.clock.probe(oldGeneration), null);
  assert.deepEqual(f.clock.estimate(oldGeneration), { status: "unavailable", reason: "stale_connection" });
  f.prime(); withinBound(f);
});

test("changing bounded drift and deterministic jitter stay within every reported interval", () => {
  let mono = 1000, server = OFFSET + mono, seed = 123456, rate = 0;
  const random = maximum => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed % maximum; };
  const advance = ms => { mono += ms; server += ms * (1 + rate / 1_000_000); };
  const clock = new RoomClock({ roomId, now: () => mono }), generation = clock.beginConnection(epoch);
  for (let n = 0; n < 250; n++) {
    rate = random(2 * L.relativeDriftPpm + 1) - L.relativeDriftPpm;
    const probe = clock.probe(generation); advance(random(80));
    const received = Math.round(server); advance(random(200));
    const sent = Math.round(server); advance(random(80));
    assert.equal(clock.accept(generation, { room_id: roomId, epoch, probe_id: probe.probe_id,
      received_at: received, sent_at: sent }), true);
    advance(random(1000));
    const estimate = clock.estimate(generation);
    if (n >= L.minimumSamples - 1) {
      assert.equal(estimate.status, "ready");
      assert.ok(Math.abs(server - estimate.server_time_ms) <= estimate.uncertainty_ms);
    }
  }
});

test("in-flight probes and retained samples are bounded and expired probes free capacity", () => {
  const f = fixture();
  const probes = Array.from({ length: L.probes }, () => f.clock.probe(f.generation));
  assert.throws(() => f.clock.probe(f.generation), error("probe_limit"));
  f.advance(L.roundTripMs + 1);
  assert.ok(f.clock.probe(f.generation));
  assert.equal(f.clock.accept(f.generation, f.reply(probes[0])), false);
  for (let n = 0; n < 20; n++) assert.equal(f.sample(), true);
  assert.equal(withinBound(f).sample_count, L.samples);
});

test("out-of-order replies use each probe's own send time", () => {
  const f = fixture(), first = f.clock.probe(f.generation);
  f.advance(5); const firstReply = f.reply(first);
  f.advance(5); const second = f.clock.probe(f.generation);
  f.advance(5); const secondReply = f.reply(second);
  f.advance(5); assert.equal(f.clock.accept(f.generation, secondReply), true);
  f.advance(80); assert.equal(f.clock.accept(f.generation, firstReply), true);
  f.sample({ outgoing: 5, incoming: 5 });
  assert.equal(withinBound(f).sample_count, 3);
});

test("an unknown old probe cannot quarantine a newly sampled epoch", () => {
  const f = fixture(); f.prime(); withinBound(f);
  assert.equal(f.clock.accept(f.generation, { room_id: roomId, epoch: uuid(99), probe_id: 999,
    received_at: Math.round(f.server()), sent_at: Math.round(f.server()) }), false);
  withinBound(f);
});

test("slow round trips and implausible server processing do not improve an estimate", () => {
  const f = fixture();
  assert.equal(f.sample({ outgoing: 1500, incoming: 501 }), false);
  assert.equal(f.sample({ processing: L.serverProcessingMs + 1 }), false);
  const probe = f.clock.probe(f.generation), received = Math.round(f.server()); f.advance(10);
  assert.equal(f.clock.accept(f.generation, f.reply(probe, received, received + 100)), false);
  assert.equal(f.estimate().status, "unavailable");
  f.prime(); withinBound(f);
});

test("samples expire even when regular estimates prevent an idle-gap reset", () => {
  const f = fixture(); f.prime(); withinBound(f);
  for (let n = 0; n < 31; n++) { f.advance(1000); f.estimate(); }
  assert.deepEqual(f.estimate(), { status: "unavailable", reason: "stale_samples" });
  f.prime(); withinBound(f);
});

test("disconnect and explicit suspend discard estimates including when monotonic time did not advance", () => {
  const f = fixture(); f.prime(); withinBound(f);
  const pending = f.clock.probe(f.generation), reply = f.reply(pending);
  f.clock.disconnect(f.generation);
  assert.deepEqual(f.estimate(), { status: "unavailable", reason: "disconnected" });
  assert.equal(f.clock.accept(f.generation, reply), false);
  f.generation = f.clock.beginConnection(epoch); f.prime(); withinBound(f);
  f.clock.suspend(); assert.deepEqual(f.estimate(), { status: "unavailable", reason: "suspended" });
  assert.equal(f.clock.probe(f.generation), null);
  f.generation = f.clock.beginConnection(epoch); assert.equal(f.estimate().status, "unavailable");
});

test("an unexplained monotonic gap requires a new connection and fresh samples", () => {
  const f = fixture(); f.prime(); withinBound(f);
  f.advance(L.idleGapMs + 1);
  assert.deepEqual(f.estimate(), { status: "unavailable", reason: "suspend_gap" });
  assert.equal(f.clock.probe(f.generation), null);
  f.generation = f.clock.beginConnection(epoch); f.prime(); withinBound(f);
});

test("incompatible server-clock jumps quarantine the connection in either direction", () => {
  for (const change of [-1000, 1000]) {
    const f = fixture(); f.prime(); withinBound(f); f.shiftServer(change);
    assert.equal(f.sample(), false);
    assert.deepEqual(f.estimate(), { status: "unavailable", reason: "clock_discontinuity" });
    f.generation = f.clock.beginConnection(epoch); f.prime(); withinBound(f);
  }
});

test("small consistent offset refinements never make the reported server time decrease", () => {
  const f = fixture(); f.prime({ outgoing: 40, incoming: 0 }); const previous = withinBound(f);
  f.sample({ outgoing: 0, incoming: 0 }); const refined = withinBound(f);
  assert.ok(refined.server_time_ms >= previous.server_time_ms);
  assert.ok(refined.uncertainty_ms <= L.uncertaintyMs);
});

test("invalid monotonic clocks fail closed without exposing callback errors", () => {
  for (const value of [NaN, Infinity, -1, Number.MAX_SAFE_INTEGER + 1, "PRIVATE"]) {
    const clock = new RoomClock({ roomId, now: () => value }), generation = clock.beginConnection(epoch);
    assert.equal(clock.probe(generation), null);
    assert.deepEqual(clock.estimate(generation), { status: "unavailable", reason: "invalid_monotonic_clock" });
  }
  const clock = new RoomClock({ roomId, now: () => { throw new Error("PRIVATE"); } }), generation = clock.beginConnection(epoch);
  assert.equal(clock.probe(generation), null); assert.doesNotMatch(JSON.stringify(clock.estimate(generation)), /PRIVATE/);
  const f = fixture(); f.prime(); withinBound(f); f.advance(-1);
  assert.deepEqual(f.estimate(), { status: "unavailable", reason: "monotonic_clock_regressed" });
});

test("clock messages reject private extras, wrong rooms, malformed timestamps and changed epochs", () => {
  const f = fixture(), probe = f.clock.probe(f.generation), reply = f.reply(probe);
  for (const change of [{ path: "PRIVATE" }, { received_at: 1.5 }, { sent_at: -1 },
    { probe_id: "1" }, { received_at: reply.sent_at + 1 }]) {
    assert.throws(() => f.clock.accept(f.generation, { ...reply, ...change }), error("invalid_sample"));
  }
  assert.throws(() => f.clock.accept(f.generation, { ...reply, room_id: uuid(99) }), error("wrong_room"));
  assert.equal(f.clock.accept(f.generation, { ...reply, epoch: uuid(98) }), false);
  assert.deepEqual(f.estimate(), { status: "unavailable", reason: "epoch_changed" });
});

test("returned estimates and probe envelopes do not expose mutable internal state", () => {
  const f = fixture(), probe = f.clock.probe(f.generation), original = { ...probe };
  probe.probe_id = 999; assert.equal(f.clock.accept(f.generation, f.reply(original)), true);
  f.prime(); const result = withinBound(f); result.server_time_ms = 1; result.sample_count = 999;
  assert.notEqual(f.estimate().server_time_ms, 1); assert.notEqual(f.estimate().sample_count, 999);
});

function authority(t) {
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
  let server = OFFSET + 1000;
  const members = new Set([owner, member]);
  const queue = new RoomQueue(storage, { roomId, ownerId: owner, epoch, now: () => server, canStart: () => true,
    authorize: actor => members.has(actor), resolveRecording: () => ({ owner_id: owner, revision: 1, state: "ready", duration_ms: 12000 }) });
  return { queue, storage, members, advance: ms => { server += ms; }, now: () => server };
}

test("authority samples include storage wait without consuming commands or queue revisions", async t => {
  const a = authority(t), before = await a.queue.snapshot(owner);
  a.storage.sync = async () => { a.advance(400); };
  const sample = await a.queue.sampleClock(owner, { epoch, probe_id: 123 });
  assert.equal(sample.sent_at - sample.received_at, 400);
  assert.deepEqual(Object.keys(sample).sort(), ["epoch", "probe_id", "received_at", "room_id", "sent_at"]);
  a.storage.sync = async () => {};
  assert.deepEqual(await a.queue.snapshot(owner), before);
  assert.doesNotMatch(JSON.stringify(sample), /path|token|credential|filename/);
});

test("authority refuses nonmembers, stale epochs, malformed probes and post-await revocation", async t => {
  const a = authority(t), probe = { epoch, probe_id: 1 };
  await assert.rejects(a.queue.sampleClock(uuid(99), probe), roomError("unavailable"));
  await assert.rejects(a.queue.sampleClock(owner, { ...probe, epoch: uuid(99) }), roomError("stale_epoch"));
  await assert.rejects(a.queue.sampleClock(owner, { ...probe, probe_id: 0 }), roomError("invalid_clock_probe"));
  await assert.rejects(a.queue.sampleClock(owner, { ...probe, token: "PRIVATE" }), roomError("invalid_fields"));
  a.storage.sync = async () => { a.members.delete(owner); };
  await assert.rejects(a.queue.sampleClock(owner, probe), roomError("unavailable"));
});

test("authority rejects storage failures, clock rollback and expiry during the timestamp exchange", async t => {
  for (const [after, code] of [
    [a => { a.advance(-1); }, "invalid_clock"],
    [a => { a.advance(ROOM_LIMITS.lifetimeMs); }, "room_expired"],
    [() => { throw new Error("PRIVATE storage details"); }, "state_unavailable"],
    [a => { a.queue.now = () => { throw new Error("PRIVATE clock details"); }; }, "state_unavailable"],
  ]) {
    const a = authority(t); a.storage.sync = async () => after(a);
    await assert.rejects(a.queue.sampleClock(owner, { epoch, probe_id: 1 }), roomError(code));
  }
});

test("real queue samples feed a clocked timeline projection through synthetic delay", async t => {
  const a = authority(t), f = fixture();
  for (let n = 0; n < L.minimumSamples; n++) {
    const probe = f.clock.probe(f.generation); f.advance(10); a.advance(10);
    a.storage.sync = async () => { f.advance(20); a.advance(20); };
    const response = await a.queue.sampleClock(owner, probe);
    a.storage.sync = async () => {}; f.advance(30); a.advance(30);
    assert.equal(f.clock.accept(f.generation, response), true);
  }
  await a.queue.command(owner, { epoch, command_id: 1, expected_revision: 0,
    action: { type: "add", occurrence_id: uuid(20), recording_id: uuid(21), explicit_selection: true } });
  await a.queue.command(owner, { epoch, command_id: 2, expected_revision: 1, action: { type: "advance" } });
  await a.queue.command(owner, { epoch, command_id: 3, expected_revision: 2,
    action: { type: "play", occurrence_id: uuid(20) } });
  f.advance(2500); a.advance(2500);
  const snapshot = await a.queue.snapshot(owner), clock = f.estimate(); assert.equal(clock.status, "ready");
  const estimated = projectTimeline(snapshot.timeline, snapshot.current, clock.server_time_ms);
  const actual = projectTimeline(snapshot.timeline, snapshot.current, a.now());
  assert.ok(Math.abs(estimated.position_ms - actual.position_ms) <= clock.uncertainty_ms);
  f.clock.disconnect(f.generation); assert.equal(f.estimate().status, "unavailable");
});

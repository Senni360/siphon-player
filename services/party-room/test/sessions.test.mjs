import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { RoomQueue, RoomError, ROOM_LIMITS } from "../src/queue.mjs";
import { SessionRoom, SESSION_LIMITS as L } from "../src/sessions.mjs";
import { RoomClient } from "../src/client.mjs";

const uuid = n => `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
const owner = uuid(1), member = uuid(2), outsider = uuid(3), roomId = uuid(4), epoch = uuid(5);
const occurrence = uuid(6), recording = uuid(7), ownerSession = uuid(10), memberSession = uuid(11), replacement = uuid(12);
const action = type => ({ type, occurrence_id: occurrence });
const error = code => cause => cause instanceof RoomError && cause.code === code;
const event = (session_id, extra = {}) => ({ epoch, session_id, ...extra });

async function fixture(t, overrides = {}, setup = true) {
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
  const members = new Set([owner, member]), connections = new Map([[ownerSession, owner], [memberSession, member], [replacement, owner]]);
  const options = { roomId, ownerId: owner, epoch, now: () => now,
    authorize: actor => members.has(actor), canStart: () => true,
    authorizeSession: (actor, id) => id === null || connections.get(id) === actor,
    resolveRecording: () => ({ owner_id: owner, revision: 1, state: "ready", duration_ms: 120000 }), ...overrides };
  const f = { db, storage, options, members, connections, now: () => now, advance: ms => { now += ms; } };
  f.restart = (Type = SessionRoom) => { f.queue = new Type(storage, options); }; f.restart();
  f.view = (actor = owner) => f.queue.snapshot(actor);
  f.packet = async (operation, actor = owner) => {
    const view = await f.view(actor);
    return { epoch, command_id: view.self.next_command_id, expected_revision: view.revision, action: operation };
  };
  f.send = async (operation, actor = owner) => f.queue.command(actor, await f.packet(operation, actor));
  f.open = (actor = owner, id = ownerSession) => f.queue.openSession(actor, event(id));
  f.select = (actor = owner, id = ownerSession, revision = 1) => f.queue.publishOutput(actor, event(id, { revision }));
  f.close = (actor = owner, id = ownerSession) => f.queue.closeSession(actor, event(id));
  f.beat = (actor = owner, id = ownerSession, revision = 1) => f.queue.heartbeatSession(actor, event(id, { revision }));
  f.report = async (actor = owner, id = ownerSession, output_revision = 1, extra = {}) => {
    const view = await f.view(actor);
    return f.queue.publishReadiness(actor, { epoch, generation: view.readiness.generation,
      revision: view.readiness.self.next_revision ?? 1, status: "ready", ...extra }, { session_id: id, output_revision });
  };
  f.ready = async () => { await f.send(action("prepare")); await f.report(); await f.report(member, memberSession); };
  f.play = async () => { await f.ready(); assert.equal((await f.send(action("play"))).accepted, true); };
  if (setup) {
    await f.open(); await f.open(member, memberSession); await f.select(); await f.select(member, memberSession);
    await f.send({ type: "add", occurrence_id: occurrence, recording_id: recording, explicit_selection: true });
    await f.send({ type: "advance" });
  }
  return f;
}

test("session IDs never authorize a connection and default, asynchronous and throwing adapters fail closed", async t => {
  for (const authorizeSession of [undefined, () => false, () => Promise.resolve(true), () => { throw new Error("PRIVATE credential"); }]) {
    const f = await fixture(t, { authorizeSession }, false);
    await assert.rejects(f.open(), error("unavailable"));
    assert.equal(f.queue.rows("SELECT COUNT(*) AS n FROM room_sessions")[0].n, 0);
    assert.equal((await f.view()).owner_presence.state, "unknown");
  }
  const f = await fixture(t, {}, false);
  await assert.rejects(f.open(member, ownerSession), error("unavailable"));
  await assert.rejects(f.open(outsider, ownerSession), error("unavailable"));
});

test("session fields are strict and foreign epochs never mutate leases", async t => {
  const f = await fixture(t, {}, false);
  for (const input of [event(ownerSession, { secret: "PRIVATE" }), event("not-an-id"), event(ownerSession, { epoch: uuid(99) })]) {
    await assert.rejects(f.queue.openSession(owner, input), error(input.epoch === epoch ? "invalid_session" : "stale_epoch"));
  }
  await f.open();
  for (const revision of [0, -1, 1.5, Number.MAX_SAFE_INTEGER, "1"]) {
    await assert.rejects(f.beat(owner, ownerSession, revision), error("invalid_session"));
  }
  assert.equal(f.queue.session(ownerSession).heartbeat_revision, 0);
});

test("opening and retrying sessions preserves the command allocator and never refreshes a lease", async t => {
  const f = await fixture(t, {}, false), first = await f.open(), before = await f.view();
  f.advance(1000); const again = await f.open();
  assert.equal(again.applied, false); assert.deepEqual(again.session, first.session);
  assert.equal((await f.view()).self.next_command_id, before.self.next_command_id);
  assert.equal((await f.view()).revision, before.revision);
  assert.equal(first.next_alarm_at, first.session.lease_until);
  await assert.rejects(f.queue.publishOwnerPresence({ epoch, revision: 99, connected: false }), error("presence_managed"));
});

test("heartbeats are ordered, duplicate-safe and hard-lifetime bounded", async t => {
  const f = await fixture(t, {}, false); const opened = await f.open();
  f.advance(5000); const fresh = await f.beat();
  assert.equal(fresh.session.lease_until, f.now() + L.leaseMs);
  f.advance(1000); assert.equal((await f.beat()).session.lease_until, fresh.session.lease_until);
  let revision = 2;
  while (f.now() + 10000 < opened.session.expires_at) { f.advance(10000); await f.beat(owner, ownerSession, revision++); }
  assert.equal(f.queue.session(ownerSession).lease_until, opened.session.expires_at);
  f.advance(opened.session.expires_at - f.now());
  await assert.rejects(f.beat(owner, ownerSession, revision), error("session_inactive"));
  const expired = await f.queue.sessionStatus(owner, event(ownerSession));
  assert.equal(expired.session.state, "expired"); assert.equal((await f.open()).applied, false);
});

test("the last owner session controls presence and late old closes cannot disconnect its replacement", async t => {
  const f = await fixture(t); await f.open(owner, replacement);
  const before = await f.view(); await f.close();
  assert.equal((await f.view()).owner_presence.state, "connected");
  assert.equal((await f.view()).owner_presence.pause_at, null);
  assert.equal((await f.close()).applied, false);
  assert.equal((await f.view()).self.next_command_id, before.self.next_command_id);
  f.advance(500); await f.close(owner, replacement);
  const departed = await f.view();
  assert.equal(departed.owner_presence.state, "disconnected");
  assert.equal(departed.owner_presence.pause_at, f.now() + ROOM_LIMITS.ownerGraceMs);
  f.advance(500); await f.close();
  assert.equal((await f.view()).owner_presence.pause_at, departed.owner_presence.pause_at);
});

test("a missed alarm freezes playback at the last owner lease expiry plus grace", async t => {
  const f = await fixture(t); await f.play();
  const anchor = (await f.view()).timeline;
  f.advance(4000); const second = await f.open(owner, replacement);
  const deadline = second.session.lease_until + ROOM_LIMITS.ownerGraceMs;
  f.advance(90000); const tick = await f.queue.tickSessions(), view = await f.view();
  assert.equal(tick.changed, true); assert.equal(view.owner_presence.pause_at, deadline);
  assert.equal(view.owner_presence.paused, true); assert.equal(view.timeline.playing, false);
  assert.equal(view.timeline.anchor_at, deadline);
  assert.equal(view.timeline.position_ms, deadline - anchor.anchor_at);
  assert.equal(view.timeline.reason, "owner_departed");
  assert.equal((await f.queue.tickSessions()).changed, false);
});

test("returning before grace cancels the deadline but returning at its boundary never auto-resumes", async t => {
  for (const atBoundary of [false, true]) {
    const f = await fixture(t); await f.play(); await f.close();
    f.advance(ROOM_LIMITS.ownerGraceMs - (atBoundary ? 0 : 1)); await f.open(owner, replacement);
    const view = await f.view(); assert.equal(view.owner_presence.state, "connected");
    assert.equal(view.owner_presence.pause_at, null); assert.equal(view.timeline.playing, !atBoundary);
    assert.equal(view.readiness.phase, "idle");
  }
});

test("room membership revocation invalidates sessions and readiness in the same revision stream", async t => {
  const f = await fixture(t); await f.ready(); const before = await f.view();
  f.members.delete(member); await f.queue.tickSessions();
  assert.equal(f.queue.session(memberSession).state, "revoked");
  assert.equal((await f.view()).readiness.ready_count, 1);
  assert.ok((await f.view()).revision > before.revision);
  assert.equal((await f.send(action("play"))).error, "readiness_required");
  f.members.add(member); assert.equal((await f.open(member, memberSession)).session.state, "revoked");
});

test("closing and expiring sessions cannot be reversed by duplicate opens or late heartbeats", async t => {
  const f = await fixture(t); await f.close();
  assert.equal((await f.open()).session.state, "closed");
  await assert.rejects(f.beat(), error("session_inactive"));
  f.advance(L.leaseMs);
  assert.equal((await f.open(member, memberSession)).session.state, "expired");
  await assert.rejects(f.beat(member, memberSession), error("session_inactive"));
});

test("two live connections per actor and eight lifetime actors bound room state", async t => {
  const f = await fixture(t, {}, false); await f.open(); await f.open(owner, replacement);
  const third = uuid(13); f.connections.set(third, owner);
  await assert.rejects(f.open(owner, third), error("session_limit"));
  await f.close(); await f.open(owner, third);
  for (let n = 0; n < L.actors - 1; n++) {
    const actor = uuid(100 + n), session = uuid(200 + n); f.members.add(actor); f.connections.set(session, actor);
    await f.open(actor, session); await f.close(actor, session);
  }
  const actor = uuid(300), session = uuid(301); f.members.add(actor); f.connections.set(session, actor);
  await assert.rejects(f.open(actor, session), error("session_actor_limit"));
});

test("closed session tombstones are bounded and retained to prevent ID reuse", async t => {
  const f = await fixture(t, {}, false);
  for (let n = 0; n < L.lifetimeSessions; n++) {
    const id = uuid(1000 + n); f.connections.set(id, owner); await f.open(owner, id); await f.close(owner, id);
  }
  const extra = uuid(2000); f.connections.set(extra, owner);
  await assert.rejects(f.open(owner, extra), error("session_limit"));
  assert.equal((await f.open(owner, uuid(1000))).session.state, "closed");
});

test("the cohort comes from live sessions, counts actors once and requires an explicit active output", async t => {
  const f = await fixture(t); await f.open(owner, replacement); await f.select(member, null, 2);
  await f.send(action("prepare"));
  const view = await f.view(member); assert.equal(view.readiness.participant_count, 2);
  assert.equal(view.readiness.self.status, "unavailable");
  await assert.rejects(f.report(member, memberSession), error("stale_output"));
  await f.report(); assert.equal((await f.send(action("play"))).error, "readiness_required");
});

test("old connections and old decoder revisions cannot report readiness after output handoff", async t => {
  const f = await fixture(t); await f.ready(); const old = await f.view();
  await f.open(owner, replacement); await f.select(owner, replacement, 2);
  let view = await f.view(); assert.equal(view.readiness.self.status, "waiting");
  assert.equal(view.readiness.ready_count, 1); assert.ok(view.revision > old.revision);
  await assert.rejects(f.report(), error("stale_output"));
  await assert.rejects(f.report(owner, replacement, 1), error("stale_output"));
  assert.equal((await f.report(owner, replacement, 2, { revision: 1 })).applied, false);
  assert.equal((await f.view()).readiness.self.status, "waiting");
  await f.report(owner, replacement, 2); assert.equal((await f.send(action("play"))).accepted, true);
});

test("switching decoder on the same session fences old readiness and never resets its sequence", async t => {
  const f = await fixture(t); await f.ready(); await f.select(owner, ownerSession, 2);
  assert.equal((await f.view()).readiness.self.next_revision, 2);
  await assert.rejects(f.report(), error("stale_output"));
  await f.report(owner, ownerSession, 2); assert.equal((await f.view()).readiness.ready_count, 2);
});

test("output selection rejects conflicting revisions, other actors and inactive sessions", async t => {
  const f = await fixture(t); await f.ready();
  await assert.rejects(f.select(owner, replacement, 1), error("output_revision_conflict"));
  await assert.rejects(f.select(owner, replacement, 2), error("session_inactive"));
  await assert.rejects(f.select(owner, memberSession, 2), error("unavailable"));
  await f.select(owner, null, 2);
  assert.equal((await f.select()).applied, false);
  assert.equal((await f.view()).readiness.self.status, "unavailable");
  await f.select(owner, ownerSession, 3);
  assert.equal((await f.view()).readiness.self.status, "waiting");
});

test("report context is mandatory and participant JSON cannot carry connection credentials", async t => {
  const f = await fixture(t); await f.send(action("prepare"));
  const report = { epoch, generation: (await f.view()).readiness.generation, revision: 1, status: "ready" };
  await assert.rejects(f.queue.publishReadiness(owner, report), error("invalid_session"));
  await assert.rejects(f.queue.publishReadiness(owner, report, { session_id: ownerSession, output_revision: 1, token: "PRIVATE" }), error("invalid_session"));
  f.connections.delete(ownerSession);
  await assert.rejects(f.report(), error("unavailable"));
  assert.equal((await f.view()).readiness.ready_count, 0);
});

test("readiness ends at its original lease boundary even if a heartbeat later extends the session", async t => {
  const f = await fixture(t); f.advance(14000); await f.ready();
  f.advance(300); await f.beat(); await f.beat(member, memberSession);
  assert.equal((await f.send(action("play"))).error, "readiness_required");
  await f.report(); await f.report(member, memberSession);
  assert.equal((await f.send(action("play"))).accepted, true);
});

test("heartbeats do not renew ready votes and session expiry cannot leave cached readiness", async t => {
  const f = await fixture(t); await f.ready(); f.advance(9000);
  await f.beat(); await f.beat(member, memberSession); f.advance(1000);
  let view = await f.view(); assert.equal(view.readiness.ready_count, 0);
  await f.report(); await f.report(member, memberSession); f.advance(14000);
  await f.queue.tickSessions(); view = await f.view();
  assert.equal(view.owner_presence.state, "disconnected"); assert.equal(view.readiness.phase, "idle");
});

test("member output replacement during playback removes admission without pausing everyone", async t => {
  const f = await fixture(t); await f.play(); const anchor = (await f.view()).timeline;
  await f.select(member, memberSession, 2);
  const view = await f.view(member); assert.equal(view.readiness.self.status, "waiting");
  assert.equal(view.readiness.phase, "started"); assert.deepEqual(view.timeline, anchor);
  await assert.rejects(f.report(member, memberSession, 2), error("load_inactive"));
});

test("late joins never enter an existing load and connection loss does not select a spare output", async t => {
  const f = await fixture(t); await f.close(member, memberSession); await f.send(action("prepare"));
  const late = uuid(20); f.connections.set(late, member); await f.open(member, late); await f.select(member, late, 2);
  assert.equal((await f.view(member)).readiness.self.included, false);
  await assert.rejects(f.report(member, late, 2), error("not_in_load"));
  await f.open(owner, replacement); await f.close();
  assert.equal((await f.view()).owner_presence.state, "connected");
  assert.equal(f.queue.outputBinding(owner), null);
  assert.equal((await f.view()).readiness.self.status, "unavailable");
});

test("durable reconstruction retains leases, output fences, report cursors and exact command receipts", async t => {
  const f = await fixture(t); await f.ready(); const packet = await f.packet(action("play"));
  const receipt = await f.queue.command(owner, packet), before = await f.view();
  f.restart(); assert.deepEqual(await f.view(), before);
  assert.deepEqual(await f.queue.command(owner, packet), receipt);
  assert.equal((await f.queue.sessionStatus(owner, event(ownerSession))).session.output_revision, 1);
  await f.select(owner, ownerSession, 2); f.restart();
  assert.equal((await f.view()).readiness.self.status, "waiting");
  assert.equal((await f.view()).self.next_command_id, before.self.next_command_id);
});

test("legacy upgrade pauses unbound playback and invalidates readiness without losing receipts", async t => {
  const f = await fixture(t, {}, false); f.db.exec("DROP TABLE room_session_state; DROP TABLE room_sessions; DROP TABLE room_outputs");
  f.restart(RoomQueue); f.queue.readinessParticipants = () => [owner];
  await f.queue.publishOwnerPresence({ epoch, revision: 1, connected: true });
  await f.send({ type: "add", occurrence_id: occurrence, recording_id: recording, explicit_selection: true });
  await f.send({ type: "advance" }); await f.send(action("prepare"));
  await f.queue.publishReadiness(owner, { epoch, generation: 1, revision: 1, status: "ready" });
  const packet = await f.packet(action("play")), receipt = await f.queue.command(owner, packet);
  const before = await f.view(); f.advance(2000); f.restart();
  const after = await f.view(); assert.equal(after.timeline.playing, false);
  assert.equal(after.timeline.reason, "owner_departed"); assert.equal(after.timeline.position_ms, 1250);
  assert.equal(after.owner_presence.state, "unknown"); assert.equal(after.readiness.phase, "idle");
  assert.equal(after.revision, before.revision + 1); assert.deepEqual(await f.queue.command(owner, packet), receipt);
});

test("a lost durable reply can be retried without extending the session lease", async t => {
  const f = await fixture(t, {}, false); const sync = f.storage.sync;
  f.storage.sync = async () => { throw new Error("PRIVATE storage"); };
  await assert.rejects(f.open(), error("state_unavailable"));
  const first = f.queue.session(ownerSession); f.storage.sync = sync; f.advance(1000); f.restart();
  const retry = await f.open(); assert.equal(retry.applied, false); assert.equal(retry.session.lease_until, first.lease_until);
});

test("transaction failures roll back session changes and readiness together", async t => {
  const f = await fixture(t); await f.ready(); const before = await f.view(), select = f.queue.maintainReadiness;
  f.queue.maintainReadiness = function(now) { const changed = select.call(this, now); if (this.output(owner).output_revision === 2) throw new Error("PRIVATE disk"); return changed; };
  await assert.rejects(f.select(owner, ownerSession, 2), error("state_unavailable"));
  f.queue.maintainReadiness = select;
  assert.equal(f.queue.output(owner).output_revision, 1); assert.deepEqual(await f.view(), before);
});

test("public snapshots and persisted client checkpoints never reveal session or output identities", async t => {
  const f = await fixture(t); await f.ready(); const snapshot = await f.view();
  const client = new RoomClient({ roomId, actorId: owner, epoch });
  const connection = client.beginConnection(); client.receiveSnapshot(connection, snapshot);
  const encoded = JSON.stringify(snapshot) + client.checkpoint();
  for (const secret of [ownerSession, memberSession, "session_id", "output_revision", "heartbeat_revision", "lease_until"]) assert.equal(encoded.includes(secret), false);
  await f.select(owner, ownerSession, 2);
  assert.equal(client.receiveSnapshot(connection, await f.view()), true);
});

test("expiry and backward clocks fail closed without renewing session state", async t => {
  const f = await fixture(t), before = f.queue.session(ownerSession);
  f.advance(-1); await assert.rejects(f.beat(), error("invalid_clock"));
  f.advance(1 + ROOM_LIMITS.lifetimeMs); await assert.rejects(f.open(owner, replacement), error("room_expired"));
  assert.deepEqual(f.queue.session(ownerSession), before);
});

test("a member's expired output loses admission while a live owner keeps the timeline running", async t => {
  const f = await fixture(t); await f.play(); f.advance(10000); await f.beat();
  const anchor = (await f.view()).timeline; f.advance(5000); await f.queue.tickSessions();
  const peer = await f.view(member);
  assert.equal(peer.readiness.self.status, "unavailable"); assert.deepEqual(peer.timeline, anchor);
  assert.equal(peer.owner_presence.state, "connected");
  await assert.rejects(f.report(member, memberSession), error("session_inactive"));
});

test("session ownership is enforced even when the connection adapter allows both actors", async t => {
  const f = await fixture(t, { authorizeSession: () => true });
  await assert.rejects(f.open(member, ownerSession), error("session_unavailable"));
  await assert.rejects(f.select(owner, memberSession, 2), error("session_inactive"));
  await assert.rejects(f.beat(owner, memberSession), error("session_inactive"));
  await assert.rejects(f.queue.sessionStatus(owner, event(memberSession)), error("session_unavailable"));
});

test("output invalidation does not erase a pending optimistic playback command", async t => {
  const f = await fixture(t); await f.ready();
  const client = new RoomClient({ roomId, actorId: owner });
  const connection = client.beginConnection(); client.receiveSnapshot(connection, await f.view());
  client.enqueueTransport(action("play"), { position_ms: 0 }); const packet = client.outbound();
  await f.select(owner, ownerSession, 2); client.receiveSnapshot(connection, await f.view());
  assert.deepEqual(client.outbound(), packet); assert.equal(client.view().pending_transport.playing, true);
  const receipt = await f.queue.command(owner, packet); assert.equal(receipt.error, "stale_revision");
  client.receiveReceipt(connection, receipt);
  assert.equal(client.view().pending_transport.state, "rejected"); assert.equal(client.view().pending_transport.playing, true);
});

import test from "node:test";
import assert from "node:assert/strict";
import { JournalError } from "../src/journal.mjs";
import { RoomError, ROOM_LIMITS } from "../src/queue.mjs";
import { SESSION_LIMITS } from "../src/sessions.mjs";
import { projectTimeline } from "../src/timeline.mjs";
import { harness, FaultWire, owner, member, epoch, add, preview, transport, occurrence } from "./support/reconnect-harness.mjs";

const frames = (h, kind, link) => h.wire.frames.filter(frame => frame.kind === kind && (!link || frame.link === link));
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };

test("two journaled participants confirm one queue and timeline after bounded clock sampling", async t => {
  const h = await harness(t); await h.play();
  const a = h.owner.client.view(), b = h.peer.client.view();
  assert.deepEqual(a.current, b.current); assert.deepEqual(a.pending, b.pending);
  assert.deepEqual(a.confirmed_timeline, b.confirmed_timeline); assert.equal(a.confirmed_timeline.playing, true);
  for (const endpoint of [h.owner, h.peer]) {
    const estimate = endpoint.clock.estimate(endpoint.link.clockGeneration);
    assert.equal(estimate.status, "ready"); assert.ok(Math.abs(estimate.server_time_ms - h.now) <= estimate.uncertainty_ms);
    assert.equal(endpoint.client.view().confirmed_readiness.self.status, "ready");
  }
  assert.deepEqual(a.intents, []); assert.deepEqual(b.intents, []);
});

test("a lost advance acknowledgement survives client reopen and authority reconstruction without advancing twice", async t => {
  const h = await harness(t); h.owner.client.enqueue({ type: "advance" });
  const original = await h.dispatch(h.owner); await h.wire.deliver(original.id);
  assert.equal((await h.room.snapshot(owner)).current.id, occurrence(2));
  for (const frame of [...h.wire.frames]) if (frame.link === h.owner.link) h.wire.drop(frame.id);
  await h.reopen(h.owner); h.restartAuthority();
  assert.equal(h.owner.client.view().mode, "offline"); assert.equal(await h.owner.client.outbound(), null);
  await h.connect(h.owner); await h.settle();
  assert.equal(h.owner.client.view().current.id, occurrence(2));
  const retry = await h.dispatch(h.owner); assert.deepEqual(retry.packet, original.packet); await h.settle();
  assert.equal((await h.room.snapshot(owner)).current.id, occurrence(2));
  assert.deepEqual(h.owner.client.view().intents, []);
});

test("duplicate requests and reordered receipts cannot erase a later optimistic seek", async t => {
  const h = await harness(t);
  h.owner.client.enqueueTransport(transport("seek", 4000), { position_ms: 4000 });
  const sent = await h.dispatch(h.owner), duplicate = h.wire.duplicate(sent.id);
  h.owner.client.enqueueTransport(transport("seek", 9000), { position_ms: 9000 });
  await h.wire.deliver(duplicate); await h.wire.deliver(sent.id);
  const receipts = frames(h, "receipt");
  assert.equal(receipts[0].encoded, receipts[1].encoded);
  await h.wire.deliver(receipts[1].id);
  assert.equal(h.owner.client.view().pending_transport.position_ms, 9000);
  await h.settle(); assert.equal(h.owner.client.view().pending_transport.position_ms, 9000);
  const second = await h.dispatch(h.owner); await h.settle();
  assert.equal(second.packet.command_id, sent.packet.command_id + 1);
  assert.equal(h.owner.client.view().confirmed_timeline.position_ms, 9000);
  assert.deepEqual(h.owner.client.view().intents, []);
});

test("an accepted receipt saved before its snapshot survives reopening without replay", async t => {
  const h = await harness(t); h.owner.client.enqueueTransport(transport("seek", 7000), { position_ms: 7000 });
  const sent = await h.dispatch(h.owner); await h.wire.deliver(sent.id);
  await h.wire.deliver(frames(h, "receipt", h.owner.link)[0].id); await h.owner.client.flush();
  assert.equal(h.owner.client.view().intents[0].state, "accepted");
  await h.reopen(h.owner); await h.connect(h.owner); await h.settle();
  assert.deepEqual(h.owner.client.view().intents, []); assert.equal(await h.dispatch(h.owner), null);
  assert.equal(h.owner.client.view().confirmed_timeline.position_ms, 7000);
});

test("old connection requests and replies are fenced across reopening", async t => {
  const h = await harness(t); h.owner.client.enqueueTransport(transport("seek", 6000), { position_ms: 6000 });
  const sent = await h.dispatch(h.owner); await h.broadcast(); const oldLink = h.owner.link;
  const oldSnapshot = frames(h, "snapshot", oldLink)[0].id;
  await h.reopen(h.owner); await h.connect(h.owner);
  assert.equal(await h.wire.deliver(sent.id), false); assert.equal(await h.wire.deliver(oldSnapshot), false);
  assert.equal((await h.room.snapshot(owner)).timeline.position_ms, 0);
  await h.settle(); const retry = await h.dispatch(h.owner); assert.deepEqual(retry.packet, sent.packet);
  await h.settle(); assert.equal(h.owner.client.view().confirmed_timeline.position_ms, 6000);
});

test("disconnect after durable packet release prevents enqueue on a replacement connection", async t => {
  const h = await harness(t); h.owner.client.enqueueTransport(transport("seek", 8000), { position_ms: 8000 });
  const released = deferred(), resume = deferred();
  const dispatch = h.dispatch(h.owner, async () => { released.resolve(); await resume.promise; });
  await released.promise; const original = await h.owner.client.outbound();
  await h.disconnect(h.owner, true); await h.connect(h.owner); resume.resolve();
  assert.equal(await dispatch, null); assert.equal(frames(h, "command").length, 0);
  await h.settle(); const retry = await h.dispatch(h.owner); assert.deepEqual(retry.packet, original);
  await h.settle();
  // The old expected revision is now stale. The intent is retained for review,
  // rather than silently rebasing a released command onto a different session.
  assert.equal(h.owner.client.view().pending_transport.error, "stale_revision");
});

test("unknown journal acknowledgements release no frame until the identical write is reconciled", async t => {
  const h = await harness(t), journal = h.owner.journal, write = journal.write.bind(journal);
  let once = true; const attempts = [];
  journal.write = async request => {
    attempts.push(structuredClone(request)); const result = await write(request);
    if (once) { once = false; throw new JournalError("journal_unavailable"); } return result;
  };
  h.owner.client.enqueueTransport(transport("seek", 3000), { position_ms: 3000 });
  await assert.rejects(h.dispatch(h.owner), error => error.code === "journal_unavailable");
  assert.equal(frames(h, "command").length, 0); assert.equal(h.owner.client.view().pending_transport.position_ms, 3000);
  const sent = await h.dispatch(h.owner); assert.deepEqual(attempts[1], attempts[0]);
  await h.settle(); assert.equal(h.owner.client.view().confirmed_timeline.position_ms, sent.packet.action.position_ms);
});

test("an old readiness callback cannot ready the replacement session and a lost play reply cannot restart the anchor", async t => {
  const h = await harness(t); await h.apply(h.owner, transport("prepare"));
  const oldReport = h.report(h.peer); h.report(h.owner); await h.wire.deliver(frames(h, "readiness", h.owner.link)[0].id);
  await h.reopen(h.peer); await h.connect(h.peer); assert.equal(await h.wire.deliver(oldReport), false); await h.settle();
  assert.equal(h.peer.client.view().confirmed_readiness.self.status, "waiting");
  h.report(h.peer); await h.settle();
  h.owner.client.enqueueTransport(transport("play"), { position_ms: 0 });
  const sent = await h.dispatch(h.owner); await h.wire.deliver(sent.id);
  const anchor = (await h.room.snapshot(owner)).timeline; assert.equal(anchor.playing, true);
  for (const frame of [...h.wire.frames]) if (frame.link === h.owner.link) h.wire.drop(frame.id);
  await h.reopen(h.owner); h.advance(2000); await h.connect(h.owner); await h.settle();
  const retry = await h.dispatch(h.owner); assert.deepEqual(retry.packet, sent.packet); await h.settle();
  assert.deepEqual(h.owner.client.view().confirmed_timeline, anchor);
  assert.notEqual(h.owner.client.view().confirmed_readiness.self.status, "ready");
  assert.equal(h.owner.clock.estimate(h.owner.link.clockGeneration).status, "unavailable");
});

test("missed owner heartbeats pause at the original deadline while the peer remains connected", async t => {
  const h = await harness(t); await h.play(); const anchor = (await h.room.snapshot(owner)).timeline;
  const lease = h.room.session(h.owner.link.session).lease_until;
  await h.disconnect(h.owner);
  for (let n = 0; n < 5; n++) { h.advance(10000); await h.heartbeat(h.peer); }
  h.restartAuthority(); await h.room.tickSessions(); await h.broadcast(); await h.settle();
  const state = h.peer.client.view(), deadline = lease + ROOM_LIMITS.ownerGraceMs;
  assert.equal(state.confirmed_timeline.anchor_at, deadline);
  assert.equal(state.confirmed_timeline.position_ms, projectTimeline(anchor, state.current, deadline).position_ms);
  assert.equal(state.confirmed_timeline.playing, false);
  assert.equal(h.room.session(h.peer.link.session).state, "live");
  await h.connect(h.owner); await h.settle();
  assert.equal(h.owner.client.view().confirmed_timeline.playing, false);
});

test("clock replies from a previous connection cannot replace fresh sampling after reconnect", async t => {
  const h = await harness(t); await h.sampleClocks(); assert.equal(h.owner.clock.estimate(h.owner.link.clockGeneration).status, "ready");
  const probe = h.sample(h.owner); h.advance(5); await h.wire.deliver(probe);
  const oldReply = frames(h, "clock", h.owner.link)[0].id;
  await h.disconnect(h.owner, true); await h.connect(h.owner);
  assert.equal(await h.wire.deliver(oldReply), false);
  assert.equal(h.owner.clock.estimate(h.owner.link.clockGeneration).status, "unavailable");
  await h.settle(); await h.sampleClocks(); assert.equal(h.owner.clock.estimate(h.owner.link.clockGeneration).status, "ready");
});

test("recreated clock counters cannot admit an old reply whose probe and generation IDs collide", async t => {
  const h = await harness(t), oldLink = h.owner.link;
  const oldProbe = h.sample(h.owner); h.advance(5); await h.wire.deliver(oldProbe);
  const oldReply = frames(h, "clock", oldLink)[0];
  await h.reopen(h.owner); await h.connect(h.owner);
  const newProbe = h.sample(h.owner), newFrame = h.wire.frames.find(frame => frame.id === newProbe);
  assert.equal(oldLink.clockGeneration, h.owner.link.clockGeneration);
  assert.equal(JSON.parse(oldReply.encoded).probe_id, JSON.parse(newFrame.encoded).probe_id);
  assert.equal(await h.wire.deliver(oldReply.id), false);
  h.advance(9); await h.wire.deliver(newProbe);
  assert.equal(await h.wire.deliver(frames(h, "clock", h.owner.link)[0].id), true);
});

test("actor identity stays outside packets and command receipts remain actor-scoped", async t => {
  const h = await harness(t); h.peer.client.enqueue(add(9, member), preview(9, member));
  const sent = await h.dispatch(h.peer); const frame = h.wire.frames.find(item => item.id === sent.id);
  frame.encoded = JSON.stringify({ ...sent.packet, actor: owner });
  await assert.rejects(h.wire.deliver(sent.id), error => error instanceof RoomError && error.code === "invalid_fields");
  assert.equal((await h.room.snapshot(member)).self.next_command_id, 1);
  const retry = await h.dispatch(h.peer); assert.deepEqual(retry.packet, sent.packet); await h.settle();
  assert.equal(h.owner.client.view().pending.at(-1).added_by, member);
  assert.equal(h.peer.client.view().intents.length, 0);
  assert.equal((await h.room.snapshot(owner)).self.next_command_id, 5);
});

test("uncertain receipt eviction remains explicit after a long partition and journal reopen", async t => {
  const h = await harness(t); h.owner.client.enqueueTransport(transport("seek", 2000), { position_ms: 2000 });
  const sent = await h.dispatch(h.owner); await h.wire.deliver(sent.id);
  for (const frame of [...h.wire.frames]) if (frame.link === h.owner.link) h.wire.drop(frame.id);
  await h.disconnect(h.owner);
  // Simulate another authorized owner device consuming the bounded receipt
  // window. This is authority input, not a claim that a disconnected wire works.
  for (let n = 0; n <= ROOM_LIMITS.receipts; n++) {
    const state = await h.room.snapshot(owner);
    await h.room.command(owner, { epoch, command_id: state.self.next_command_id, expected_revision: state.revision,
      action: { type: "remove", occurrence_id: occurrence(99) } });
  }
  h.advance(SESSION_LIMITS.leaseMs); await h.reopen(h.owner); await h.connect(h.owner); await h.settle();
  assert.equal(h.owner.client.view().pending_transport.state, "uncertain");
  assert.equal(await h.dispatch(h.owner), null); assert.equal(frames(h, "command").length, 0);
  const id = h.owner.client.view().pending_transport.intent_id;
  h.owner.client.acceptUncertainOutcome(id); await h.owner.client.flush();
  assert.equal(h.owner.client.view().pending_transport, null);
});

test("the fault harness itself bounds queued frames, encoded size and delivery work", async () => {
  const wire = new FaultWire(); const id = wire.push("fixture", null, { n: 1 }, () => true);
  for (let n = 1; n < 64; n++) wire.duplicate(id);
  assert.throws(() => wire.duplicate(id), /fixture_buffer_limit/); assert.equal(wire.frames.length, 64);
  await wire.drain();
  assert.throws(() => wire.push("fixture", null, "x".repeat(65537), () => true), /fixture_frame_limit/);
  assert.equal(wire.frames.length, 0);
});

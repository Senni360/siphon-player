import test from "node:test";
import assert from "node:assert/strict";
import { RoomConnection, ConnectionError, CONNECTION_LIMITS } from "../src/connection.mjs";
import { JournalError } from "../src/journal.mjs";
import { harness, owner, transport, occurrence, uuid } from "./support/reconnect-harness.mjs";

const frame = (kind, payload) => JSON.stringify({ kind, payload });
const error = code => cause => cause instanceof ConnectionError && cause.code === code;
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };

async function fixture(t) {
  const h = await harness(t); await h.owner.client.flush();
  const journal = h.owner.journal, identity = h.owner.identity;
  const options = { now: () => h.now - 1_800_000_000_000 };
  const adapter = await RoomConnection.open(identity, journal, options);
  const f = { h, journal, identity, options, adapter, sent: [] };
  f.attach = (enqueue = encoded => { f.sent.push(encoded); return true; }) => {
    f.handle = adapter.attach(enqueue); return f.handle;
  };
  f.snapshot = async (handle = f.handle) => handle.receive(frame("snapshot", await h.room.snapshot(owner)));
  f.seek = position => adapter.enqueueTransport(transport("seek", position), { position_ms: position });
  f.attach(); await f.snapshot(); return f;
}

test("fresh authorized snapshots gate both command dispatch and clock sampling", async t => {
  const f = await fixture(t); f.seek(1000); const handle = f.attach();
  assert.equal(f.adapter.view().mode, "syncing"); assert.equal(await f.adapter.sendCommand(), "waiting");
  assert.equal(f.adapter.sendProbe(), "waiting"); assert.equal(f.sent.length, 0);
  await f.snapshot(handle); assert.equal(await f.adapter.sendCommand(), "sent");
  assert.equal(JSON.parse(f.sent[0]).payload.action.position_ms, 1000);
});

test("concurrent dispatch callers share one durable write and one synchronous enqueue", async t => {
  const f = await fixture(t); f.seek(2000);
  const first = f.adapter.sendCommand(), second = f.adapter.sendCommand(); assert.equal(first, second);
  assert.equal(await first, "sent"); assert.equal(f.sent.length, 1);
  const saved = await f.journal.read();
  assert.equal(JSON.parse(saved.checkpoint).intents[0].packet.command_id, JSON.parse(f.sent[0]).payload.command_id);
  assert.equal(f.adapter.view().journal.state, "saved");
});

test("backpressure retains the exact durable command without unnecessary journal writes", async t => {
  const f = await fixture(t); let accept = false; const attempts = [];
  f.attach(encoded => { attempts.push(encoded); return accept; }); await f.snapshot(); f.seek(3000);
  assert.equal(await f.adapter.sendCommand(), "backpressure"); const saved = await f.journal.read();
  assert.equal(f.adapter.view().pending_transport.position_ms, 3000);
  accept = true; assert.equal(await f.adapter.sendCommand(), "sent");
  assert.equal(attempts[1], attempts[0]); assert.deepEqual(await f.journal.read(), saved);
});

test("disconnect during journal durability cannot dispatch to a replacement transport", async t => {
  const f = await fixture(t), entered = deferred(), release = deferred(), write = f.journal.write.bind(f.journal);
  let once = true;
  f.journal.write = async request => { if (once) { once = false; entered.resolve(); await release.promise; } return write(request); };
  f.seek(4000); const pending = f.adapter.sendCommand(); await entered.promise;
  const old = f.handle; f.attach(); await f.snapshot(); release.resolve();
  assert.equal(await pending, "stale"); assert.equal(f.sent.length, 0); assert.equal(old.close(), false);
  assert.equal(await f.adapter.sendCommand(), "sent"); assert.equal(f.sent.length, 1);
});

test("old callbacks are ignored before decoding and cannot poison a replacement connection", async t => {
  const f = await fixture(t), old = f.handle; f.seek(5000); f.attach(); await f.snapshot();
  assert.equal(old.receive("PRIVATE invalid JSON"), false);
  assert.equal(old.receive("x".repeat(CONNECTION_LIMITS.frameBytes + 1)), false);
  assert.equal(old.close(), false); assert.equal(f.adapter.view().mode, "ready");
  assert.equal(f.adapter.view().connection.error, null); assert.equal(f.adapter.view().pending_transport.position_ms, 5000);
});

test("a transport throw preserves uncertain dispatch and never exposes its private error", async t => {
  const f = await fixture(t); let attempted;
  f.attach(encoded => { attempted = encoded; throw new Error("PRIVATE provider grant"); }); await f.snapshot(); f.seek(6000);
  await assert.rejects(f.adapter.sendCommand(), error("transport_failed"));
  assert.equal(f.adapter.view().mode, "offline"); assert.equal(f.adapter.view().connection.error, "transport_failed");
  assert.doesNotMatch(JSON.stringify(f.adapter.view()), /PRIVATE|grant/);
  f.attach(); await f.snapshot(); assert.equal(await f.adapter.sendCommand(), "sent"); assert.equal(f.sent[0], attempted);
});

test("asynchronous senders are refused without renumbering the uncertain packet", async t => {
  const f = await fixture(t); let attempted;
  f.attach(encoded => { attempted = encoded; return Promise.reject(new Error("PRIVATE sender")); }); await f.snapshot(); f.seek(7000);
  await assert.rejects(f.adapter.sendCommand(), error("async_transport"));
  f.attach(); await f.snapshot(); await f.adapter.sendCommand(); assert.equal(f.sent[0], attempted);
});

test("an old sender failing after reentrant replacement cannot close or mark the new connection", async t => {
  const f = await fixture(t); let next;
  f.attach(() => { next = f.adapter.attach(() => true); throw new Error("PRIVATE former socket"); }); await f.snapshot(); f.seek(8000);
  await assert.rejects(f.adapter.sendCommand(), error("transport_failed"));
  assert.equal(f.adapter.view().connection.attached, true); assert.equal(f.adapter.view().connection.error, null);
  assert.equal(f.adapter.view().mode, "syncing"); await f.snapshot(next);
  assert.equal(await f.adapter.sendCommand(), "sent");
});

test("invalid live frames fail closed while preserving pending optimistic intent", async t => {
  for (const encoded of ["not JSON PRIVATE", frame("unknown", {}), JSON.stringify({ kind: "receipt", payload: {}, token: "PRIVATE" }),
    frame("snapshot", { token: "PRIVATE" }), frame("failure", { command_id: 5, code: "PRIVATE error" })]) {
    const f = await fixture(t); f.seek(9000); await f.adapter.sendCommand();
    assert.throws(() => f.handle.receive(encoded), cause => cause instanceof ConnectionError && !cause.message.includes("PRIVATE"));
    assert.equal(f.adapter.view().mode, "offline"); assert.equal(f.adapter.view().pending_transport.position_ms, 9000);
  }
});

test("inbound frame limits count UTF-8 bytes and reject non-string input", async t => {
  const f = await fixture(t);
  assert.throws(() => f.handle.receive(frame("snapshot", "é".repeat(32768))), error("frame_limit"));
  f.attach(); assert.throws(() => f.handle.receive({ kind: "snapshot" }), error("invalid_frame"));
});

test("clock counters may collide after replacement but old handles never deliver their replies", async t => {
  const f = await fixture(t), old = f.handle;
  assert.equal(f.adapter.sendProbe(), "sent"); const oldProbe = JSON.parse(f.sent.pop()).payload;
  const oldReply = await f.h.room.sampleClock(owner, oldProbe);
  f.attach(); await f.snapshot(); f.adapter.sendProbe(); const freshProbe = JSON.parse(f.sent.pop()).payload;
  assert.equal(freshProbe.probe_id, oldProbe.probe_id);
  assert.equal(old.receive(frame("clock", oldReply)), false);
  f.h.advance(5); assert.equal(f.handle.receive(frame("clock", await f.h.room.sampleClock(owner, freshProbe))), true);
  assert.equal(f.adapter.view().connection.clock.status, "unavailable");
  for (let n = 0; n < 2; n++) {
    f.adapter.sendProbe(); const probe = JSON.parse(f.sent.pop()).payload;
    f.h.advance(5); f.handle.receive(frame("clock", await f.h.room.sampleClock(owner, probe)));
  }
  assert.equal(f.adapter.view().connection.clock.status, "ready");
});

test("probe backpressure remains bounded and recovers after unanswered probes expire", async t => {
  const f = await fixture(t); f.attach(() => false); await f.snapshot();
  for (let n = 0; n < 4; n++) assert.equal(f.adapter.sendProbe(), "backpressure");
  assert.equal(f.adapter.sendProbe(), "limited"); f.h.advance(2001);
  assert.equal(f.adapter.sendProbe(), "backpressure");
});

test("suspension and room-level failures invalidate clock state and require fresh state", async t => {
  const f = await fixture(t); f.seek(10000); await f.adapter.sendCommand(); const packet = JSON.parse(f.sent[0]).payload;
  f.handle.receive(frame("failure", { command_id: packet.command_id, code: "state_unavailable" }));
  assert.equal(f.adapter.view().mode, "syncing"); assert.equal(f.adapter.sendProbe(), "waiting");
  assert.equal(await f.adapter.sendCommand(), "waiting"); await f.snapshot();
  assert.equal(f.adapter.suspend(), true); assert.equal(f.adapter.view().mode, "offline");
  assert.equal(f.adapter.view().connection.clock.status, "unavailable");
  f.attach(); await f.snapshot(); assert.equal(await f.adapter.sendCommand(), "sent"); assert.equal(f.sent[1], f.sent[0]);
});

test("epoch changes quarantine pending intent and do not initialize a new playback clock", async t => {
  const f = await fixture(t); f.seek(11000); const snapshot = await f.h.room.snapshot(owner);
  assert.equal(f.handle.receive(frame("snapshot", { ...snapshot, epoch: uuid(9999) })), false);
  assert.equal(f.adapter.view().blocked, "epoch_changed"); assert.equal(f.adapter.sendProbe(), "waiting");
  assert.equal(await f.adapter.sendCommand(), "waiting"); assert.equal(f.adapter.view().pending_transport.position_ms, 11000);
});

test("journal failures release no transport frame and retry the original pending command", async t => {
  const f = await fixture(t), write = f.journal.write.bind(f.journal); let once = true;
  f.journal.write = async request => { if (once) { once = false; throw new JournalError("journal_unavailable"); } return write(request); };
  f.seek(12000); await assert.rejects(f.adapter.sendCommand(), cause => cause.code === "journal_unavailable");
  assert.equal(f.sent.length, 0); assert.equal(f.adapter.view().journal.state, "failed");
  assert.equal(await f.adapter.sendCommand(), "sent"); assert.equal(f.adapter.view().journal.state, "saved");
});

test("the adapter runs through the fault harness and retries a lost advance receipt once", async t => {
  const f = await fixture(t), h = f.h; let active;
  const attachWire = () => {
    const handle = f.adapter.attach(encoded => {
      const message = JSON.parse(encoded);
      h.wire.push(message.kind, handle, message.payload, async packet => {
        if (handle !== active) return false;
        const receipt = await h.room.command(owner, packet);
        h.wire.push("receipt", handle, receipt, value => handle.receive(frame("receipt", value)));
        h.wire.push("snapshot", handle, await h.room.snapshot(owner), value => handle.receive(frame("snapshot", value)));
        return true;
      }); return true;
    });
    active = handle; return handle;
  };
  let handle = attachWire(); await f.snapshot(handle); f.adapter.enqueue({ type: "advance" });
  await f.adapter.sendCommand(); const original = h.wire.frames[0].encoded; await h.wire.deliver();
  for (const queued of [...h.wire.frames]) h.wire.drop(queued.id);
  h.restartAuthority(); handle = attachWire(); await f.snapshot(handle);
  await f.adapter.sendCommand(); assert.equal(h.wire.frames[0].encoded, original); await h.settle();
  assert.equal((await h.room.snapshot(owner)).current.id, occurrence(2));
  assert.deepEqual(f.adapter.view().intents, []);
});

test("reopening a connection adapter restores packets but never a live handle or clock", async t => {
  const f = await fixture(t); f.seek(13000); await f.adapter.sendCommand(); const packet = f.sent[0];
  f.handle.close(); await f.adapter.flush();
  const recovered = await RoomConnection.open(f.identity, f.journal, f.options), sent = [];
  assert.equal(recovered.view().mode, "offline"); assert.equal(await recovered.sendCommand(), "waiting");
  const handle = recovered.attach(encoded => { sent.push(encoded); return true; });
  assert.equal(recovered.sendProbe(), "waiting"); handle.receive(frame("snapshot", await f.h.room.snapshot(owner)));
  assert.equal(await recovered.sendCommand(), "sent"); assert.equal(sent[0], packet);
  assert.equal(recovered.view().connection.clock.status, "unavailable");
});

test("failure messages are validated even when no matching command is pending", async t => {
  const f = await fixture(t);
  for (const payload of [{ command_id: 0, code: "unavailable" }, { command_id: 1, code: "PRIVATE" }, { command_id: "1", code: "unavailable" }]) {
    f.attach(); await f.snapshot(); assert.throws(() => f.handle.receive(frame("failure", payload)), error("invalid_frame"));
  }
});

test("enqueue must report literal queue acceptance and never arbitrary transport metadata", async t => {
  const f = await fixture(t); f.seek(14000);
  for (const returned of [undefined, 1, { private: "PRIVATE" }]) {
    f.attach(() => returned); await f.snapshot(); await assert.rejects(f.adapter.sendCommand(), error("invalid_transport"));
    assert.equal(f.adapter.view().pending_transport.position_ms, 14000);
  }
});

test("identity is copied before awaiting journal recovery", async t => {
  const f = await fixture(t); f.handle.close(); await f.adapter.flush();
  const identity = { ...f.identity }, pending = RoomConnection.open(identity, f.journal, f.options);
  identity.roomId = uuid(9999); identity.actorId = uuid(9998);
  const recovered = await pending, sent = [], handle = recovered.attach(encoded => { sent.push(encoded); return true; });
  handle.receive(frame("snapshot", await f.h.room.snapshot(owner))); assert.equal(recovered.sendProbe(), "sent");
  const reply = await f.h.room.sampleClock(owner, JSON.parse(sent[0]).payload);
  assert.equal(handle.receive(frame("clock", reply)), true);
});

test("same-connection resynchronization never reuses an outstanding old clock probe ID", async t => {
  const f = await fixture(t); f.adapter.sendProbe(); const oldProbe = JSON.parse(f.sent.pop()).payload;
  const oldReply = await f.h.room.sampleClock(owner, oldProbe);
  f.seek(15000); await f.adapter.sendCommand(); const command = JSON.parse(f.sent.pop()).payload;
  f.handle.receive(frame("failure", { command_id: command.command_id, code: "state_unavailable" }));
  await f.snapshot(); f.adapter.sendProbe(); const freshProbe = JSON.parse(f.sent.pop()).payload;
  assert.notEqual(freshProbe.probe_id, oldProbe.probe_id);
  assert.equal(f.handle.receive(frame("clock", oldReply)), false);
  assert.equal(f.handle.receive(frame("clock", await f.h.room.sampleClock(owner, freshProbe))), true);
});

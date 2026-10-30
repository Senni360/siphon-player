// Test-only transport rehearsal. No sockets, credentials, timers or audio.
// Connection objects are fake authenticated host context, never packet fields.
import { DatabaseSync } from "node:sqlite";
import { SessionRoom } from "../../src/sessions.mjs";
import { JournaledRoomClient } from "../../src/journal.mjs";
import { SqliteRoomJournal } from "../../src/sqlite-journal.mjs";
import { RoomClock } from "../../src/clock.mjs";

export const uuid = n => `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
export const owner = uuid(1), member = uuid(2), roomId = uuid(3), epoch = uuid(4);
export const occurrence = n => uuid(100 + n), recording = actor => actor === owner ? uuid(10) : uuid(11);
export const add = (n, actor = owner) => ({ type: "add", occurrence_id: occurrence(n), recording_id: recording(actor), explicit_selection: true });
export const preview = (n, actor = owner) => ({ id: occurrence(n), recording_id: recording(actor), added_by: actor,
  state: "ready", duration_ms: 120000, media_revision: 1, source: "shared_local" });
export const transport = (type, position_ms) => ({ type, occurrence_id: occurrence(1), ...(position_ms === undefined ? {} : { position_ms }) });

export class FaultWire {
  frames = [];
  next = 1;
  push(kind, link, body, receive) {
    const encoded = JSON.stringify(body);
    if (Buffer.byteLength(encoded) > 64 * 1024) throw new Error("fixture_frame_limit");
    if (this.frames.length >= 64) throw new Error("fixture_buffer_limit");
    const frame = { id: this.next++, kind, link, encoded, receive }; this.frames.push(frame);
    return frame.id;
  }
  duplicate(id) {
    const frame = this.frames.find(item => item.id === id);
    if (!frame) throw new Error("fixture_missing_frame");
    return this.push(frame.kind, frame.link, JSON.parse(frame.encoded), frame.receive);
  }
  drop(id) { this.frames = this.frames.filter(frame => frame.id !== id); }
  async deliver(id = this.frames[0]?.id) {
    const index = this.frames.findIndex(frame => frame.id === id);
    if (index < 0) throw new Error("fixture_missing_frame");
    const [frame] = this.frames.splice(index, 1);
    return frame.receive(JSON.parse(frame.encoded));
  }
  async drain() {
    let remaining = 256;
    while (this.frames.length) {
      if (!remaining--) throw new Error("fixture_delivery_limit");
      await this.deliver();
    }
  }
}

function storage(db) {
  return {
    sql: { exec(query, ...parameters) {
      const statement = db.prepare(query);
      const rows = statement.columns().length ? statement.all(...parameters) : (statement.run(...parameters), []);
      return { toArray: () => rows.map(row => ({ ...row })) };
    } },
    transactionSync(operation) {
      db.exec("BEGIN");
      try { const result = operation(); db.exec("COMMIT"); return result; }
      catch (error) { db.exec("ROLLBACK"); throw error; }
    }, async sync() {},
  };
}

export class ReconnectHarness {
  now = 1_800_000_000_000;
  wire = new FaultWire();
  links = new Map();
  participants = new Map();
  members = new Set([owner, member]);
  nextSession = 2000;
  constructor(t) {
    const db = new DatabaseSync(":memory:"); t.after(() => db.close());
    this.storage = storage(db);
    this.options = { roomId, ownerId: owner, epoch, now: () => this.now, canStart: () => true,
      authorize: actor => this.members.has(actor),
      authorizeSession: (actor, id) => this.links.get(id)?.active === true && this.links.get(id)?.actor === actor,
      resolveRecording: (actor, id) => id === recording(actor)
        ? { owner_id: actor, revision: 1, state: "ready", duration_ms: 120000 } : null };
    this.restartAuthority(); this.test = t;
  }
  restartAuthority() { this.room = new SessionRoom(this.storage, this.options); }
  advance(ms) { this.now += ms; }
  async participant(actor, offset = 0) {
    const db = new DatabaseSync(":memory:"); this.test.after(() => db.close());
    const identity = { roomId, actorId: actor }, journal = new SqliteRoomJournal(db, identity, { create: true });
    const endpoint = { actor, identity, journal, offset, link: null,
      client: await JournaledRoomClient.open(identity, journal),
      clock: new RoomClock({ roomId, now: () => this.now - 1_800_000_000_000 + offset }) };
    this.participants.set(actor, endpoint); return endpoint;
  }
  async connect(endpoint) {
    if (endpoint.link?.active) throw new Error("fixture_already_connected");
    const session = uuid(this.nextSession++), generation = endpoint.client.beginConnection();
    const link = { actor: endpoint.actor, session, generation, clockGeneration: endpoint.clock.beginConnection(epoch),
      active: true, heartbeat: 0, output: 0, endpoint };
    this.links.set(session, link); endpoint.link = link;
    const opened = await this.room.openSession(link.actor, { epoch, session_id: session });
    link.output = opened.session.next_output_revision;
    await this.room.publishOutput(link.actor, { epoch, session_id: session, revision: link.output });
    await this.broadcast(); return link;
  }
  async disconnect(endpoint, graceful = false) {
    const link = endpoint.link;
    if (!link?.active) return;
    if (graceful) await this.room.closeSession(link.actor, { epoch, session_id: link.session });
    link.active = false; endpoint.client.disconnect(link.generation); endpoint.clock.disconnect(link.clockGeneration);
  }
  async reopen(endpoint) {
    // Recreate only the client core from its existing SQLite journal. This is
    // deliberately not described as an OS crash or a second computer.
    await this.disconnect(endpoint);
    endpoint.client = await JournaledRoomClient.open(endpoint.identity, endpoint.journal);
    endpoint.clock = new RoomClock({ roomId, now: () => this.now - 1_800_000_000_000 + endpoint.offset });
    endpoint.link = null;
  }
  current(link) {
    return link?.active && link.endpoint.link === link && this.members.has(link.actor);
  }
  reply(link, kind, payload) {
    return this.wire.push(kind, link, payload, value => {
      // Clock generations/probe IDs are instance-local and restart at one.
      // Fence the host connection before consulting any reconstructed core.
      if (!this.current(link)) return false;
      // The current core receives the original connection generation, including
      // after reopen. Old replies cannot be relabeled as new-connection state.
      const client = link.endpoint.client;
      if (kind === "snapshot") return client.receiveSnapshot(link.generation, value);
      if (kind === "receipt") return client.receiveReceipt(link.generation, value);
      if (kind === "clock") return link.endpoint.clock.accept(link.clockGeneration, value);
      throw new Error("fixture_unknown_reply");
    });
  }
  async broadcast() {
    for (const endpoint of this.participants.values()) {
      const link = endpoint.link;
      if (this.current(link)) this.reply(link, "snapshot", await this.room.snapshot(link.actor));
    }
  }
  async dispatch(endpoint, beforeSend = async () => {}) {
    const link = endpoint.link, packet = await endpoint.client.outbound();
    if (!packet) return null;
    await beforeSend();
    // This extra host check covers disconnect AFTER journal.outbound resolves.
    // A future socket adapter must perform it at the actual enqueue boundary.
    if (!this.current(link)) return null;
    const id = this.wire.push("command", link, packet, async value => {
      if (!this.current(link)) return false;
      const receipt = await this.room.command(link.actor, value);
      this.reply(link, "receipt", receipt); await this.broadcast(); return true;
    });
    return { id, packet };
  }
  report(endpoint, status = "ready") {
    const link = endpoint.link, state = endpoint.client.view().confirmed_readiness;
    const event = { epoch, generation: state.generation, revision: state.self.next_revision, status };
    const context = { session_id: link.session, output_revision: link.output };
    return this.wire.push("readiness", link, event, async value => {
      if (!this.current(link)) return false;
      await this.room.publishReadiness(link.actor, value, context); await this.broadcast(); return true;
    });
  }
  sample(endpoint) {
    const link = endpoint.link, probe = endpoint.clock.probe(link.clockGeneration);
    if (!probe) return null;
    return this.wire.push("probe", link, probe, async value => {
      if (!this.current(link)) return false;
      this.reply(link, "clock", await this.room.sampleClock(link.actor, value)); return true;
    });
  }
  async sampleClocks() {
    for (let n = 0; n < 3; n++) {
      for (const endpoint of this.participants.values()) this.sample(endpoint);
      this.advance(7); await this.wire.drain(); this.advance(11);
    }
  }
  async heartbeat(endpoint) {
    const link = endpoint.link;
    if (!this.current(link)) return;
    await this.room.heartbeatSession(link.actor, { epoch, session_id: link.session, revision: ++link.heartbeat });
  }
  async settle() { await this.wire.drain(); }
  async apply(endpoint, operation, value) {
    if (["prepare", "play", "pause", "seek", "start_ready"].includes(operation.type)) endpoint.client.enqueueTransport(operation, value ?? { position_ms: 0 });
    else endpoint.client.enqueue(operation, value);
    const sent = await this.dispatch(endpoint); if (!sent) throw new Error("fixture_not_dispatchable");
    await this.settle(); return sent;
  }
  async seed() {
    this.owner = await this.participant(owner, 1234); this.peer = await this.participant(member, 4567);
    await this.connect(this.owner); await this.connect(this.peer); await this.settle();
    for (let n = 1; n <= 3; n++) await this.apply(this.owner, add(n), preview(n));
    await this.apply(this.owner, { type: "advance" }); return this;
  }
  async play() {
    await this.sampleClocks(); await this.apply(this.owner, transport("prepare"));
    this.report(this.owner); this.report(this.peer); await this.settle();
    await this.apply(this.owner, transport("play"));
  }
}

export async function harness(t) { return new ReconnectHarness(t).seed(); }

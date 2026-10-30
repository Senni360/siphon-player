// Headless connection boundary. The host owns authentication, socket lifetime,
// bounded synchronous enqueue, timers and native output. No socket is opened.
import { JournaledRoomClient } from "./journal.mjs";
import { RoomClock } from "./clock.mjs";

export const CONNECTION_LIMITS = Object.freeze({ frameBytes: 64 * 1024 });
const CREATE = Symbol("room-connection"), encoder = new TextEncoder();
const KINDS = new Set(["snapshot", "receipt", "clock", "failure"]);
const FAILURES = new Set(["receipt_expired", "command_conflict", "stale_epoch", "room_expired",
  "actor_limit", "unavailable", "command_gap", "state_unavailable"]);
export class ConnectionError extends Error {
  constructor(code) { super(code); this.code = code; }
}
function requireThat(value, code) { if (!value) throw new ConnectionError(code); }
function decode(encoded) {
  requireThat(typeof encoded === "string", "invalid_frame");
  // UTF-8 uses at least one byte per UTF-16 code unit. Reject huge strings
  // before allocating their encoded copy, then check the actual byte count.
  requireThat(encoded.length <= CONNECTION_LIMITS.frameBytes, "frame_limit");
  requireThat(encoder.encode(encoded).length <= CONNECTION_LIMITS.frameBytes, "frame_limit");
  let value;
  try { value = JSON.parse(encoded); } catch { throw new ConnectionError("invalid_frame"); }
  requireThat(value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).sort().join(",") === "kind,payload" && KINDS.has(value.kind), "invalid_frame");
  return value;
}

export class RoomConnection {
  #client;
  #roomId;
  #now;
  #active = null;
  #sending = null;
  #error = null;
  constructor(client, identity, now, token) {
    requireThat(token === CREATE, "invalid_connection");
    this.#client = client; this.#roomId = identity.roomId; this.#now = now;
  }
  static async open(identity, journal, { now = () => performance.now() } = {}) {
    requireThat(typeof now === "function", "invalid_connection");
    const bound = { roomId: identity?.roomId, actorId: identity?.actorId };
    return new RoomConnection(await JournaledRoomClient.open(bound, journal), bound, now, CREATE);
  }
  attach(enqueue) {
    requireThat(typeof enqueue === "function", "invalid_transport");
    if (this.#active) this.#detach(this.#active);
    const link = { generation: this.#client.beginConnection(), enqueue, clock: null, clockGeneration: null };
    this.#active = link; this.#error = null;
    // These closures retain this exact instance/connection. Neither counters
    // nor incoming JSON may retarget an old callback to the current connection.
    return Object.freeze({ receive: encoded => this.#receive(link, encoded), close: () => this.#detach(link) });
  }
  #detach(link) {
    if (link !== this.#active) return false;
    this.#active = null; this.#client.disconnect(link.generation);
    link.clock?.disconnect(link.clockGeneration); return true;
  }
  #updateClock(link) {
    const view = this.#client.view();
    if (view.mode !== "ready") {
      link.clock?.disconnect(link.clockGeneration); link.clockGeneration = null;
    } else if (link.clockGeneration === null) {
      // Retain probe high-water marks during resync on this same transport.
      // Recreating the clock here could admit a delayed pre-resync reply.
      link.clock ??= new RoomClock({ roomId: this.#roomId, now: this.#now });
      link.clockGeneration = link.clock.beginConnection(view.epoch);
    }
  }
  #receive(link, encoded) {
    if (link !== this.#active) return false;
    try {
      const { kind, payload } = decode(encoded); let applied;
      if (kind === "snapshot") applied = this.#client.receiveSnapshot(link.generation, payload);
      else if (kind === "receipt") applied = this.#client.receiveReceipt(link.generation, payload);
      else if (kind === "failure") {
        requireThat(payload && Object.keys(payload).sort().join(",") === "code,command_id"
          && Number.isSafeInteger(payload.command_id) && payload.command_id > 0
          && payload.command_id < Number.MAX_SAFE_INTEGER && FAILURES.has(payload.code), "invalid_frame");
        applied = this.#client.receiveFailure(link.generation, payload.command_id, payload.code);
      } else return link.clockGeneration !== null && this.#client.view().mode === "ready"
        ? link.clock.accept(link.clockGeneration, payload) : false;
      this.#updateClock(link); return applied;
    } catch (error) {
      const normalized = error instanceof ConnectionError ? error : new ConnectionError("invalid_message");
      if (link === this.#active) { this.#error = normalized.code; this.#detach(link); }
      throw normalized;
    }
  }
  #enqueue(link, kind, payload) {
    if (link !== this.#active) return "stale";
    try {
      const encoded = JSON.stringify({ kind, payload });
      requireThat(encoder.encode(encoded).length <= CONNECTION_LIMITS.frameBytes, "frame_limit");
      const accepted = link.enqueue(encoded);
      if (accepted && typeof accepted.then === "function") {
        // A forbidden async sender may already have queued bytes. Preserve the
        // original packet and consume rejection without trusting the outcome.
        Promise.resolve(accepted).catch(() => {});
        throw new ConnectionError("async_transport");
      }
      requireThat(typeof accepted === "boolean", "invalid_transport");
      if (link !== this.#active) return "stale";
      return accepted ? "sent" : "backpressure";
    } catch (error) {
      const normalized = error instanceof ConnectionError ? error : new ConnectionError("transport_failed");
      if (link === this.#active) { this.#error = normalized.code; this.#detach(link); }
      throw normalized;
    }
  }
  sendCommand() {
    if (this.#sending) return this.#sending;
    const link = this.#active;
    if (!link || this.#client.view().mode !== "ready") return Promise.resolve("waiting");
    this.#sending = (async () => {
      const packet = await this.#client.outbound();
      if (link !== this.#active) return "stale";
      if (!packet || this.#client.view().mode !== "ready") return "waiting";
      // No await between this connection check and its synchronous enqueue.
      return this.#enqueue(link, "command", packet);
    })().finally(() => { this.#sending = null; });
    return this.#sending;
  }
  sendProbe() {
    const link = this.#active;
    if (!link || link.clockGeneration === null || this.#client.view().mode !== "ready") return "waiting";
    let probe;
    try { probe = link.clock.probe(link.clockGeneration); }
    catch (error) { if (error.code === "probe_limit") return "limited"; throw new ConnectionError("clock_unavailable"); }
    return probe ? this.#enqueue(link, "probe", probe) : "waiting";
  }
  suspend() { return this.#active ? this.#detach(this.#active) : false; }
  enqueue(...args) { return this.#client.enqueue(...args); }
  enqueueTransport(...args) { return this.#client.enqueueTransport(...args); }
  discard(id) { return this.#client.discard(id); }
  retryRejected(id) { return this.#client.retryRejected(id); }
  acceptUncertainOutcome(id) { return this.#client.acceptUncertainOutcome(id); }
  flush() { return this.#client.flush(); }
  view() {
    const link = this.#active;
    return { ...this.#client.view(), connection: { attached: !!link, sending: !!this.#sending, error: this.#error,
      clock: link && link.clockGeneration !== null ? link.clock.estimate(link.clockGeneration)
        : { status: "unavailable", reason: link ? "needs_snapshot" : "disconnected" } } };
  }
}

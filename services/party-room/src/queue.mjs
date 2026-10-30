// Isolated room authority. No HTTP/WebSocket handler, playback or credentials.
// Membership and media metadata must come from synchronous trusted adapters.
import { TIMELINE_LIMITS, selectedTimeline, pausedTimeline, projectTimeline, validateTimeline } from "./timeline.mjs";
export const ROOM_LIMITS = Object.freeze({ pending: 32, occurrences: 256, actors: 8, receipts: 64,
  lifetimeMs: 24 * 60 * 60 * 1000, ownerGraceMs: 30_000 });
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const STATES = new Set(["unavailable", "uploading", "ready"]);

export class RoomError extends Error {
  constructor(code) { super(code); this.code = code; }
}
function requireThat(value, code) { if (!value) throw new RoomError(code); }
function uuid(value) { return typeof value === "string" && UUID.test(value); }
function integer(value, min = 0) { return Number.isSafeInteger(value) && value >= min; }
function fields(value, names) {
  requireThat(value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).sort().join(",") === [...names].sort().join(","), "invalid_fields");
}
function action(input) {
  requireThat(input && typeof input === "object", "invalid_action");
  if (input.type === "add") {
    fields(input, ["type", "occurrence_id", "recording_id", "explicit_selection"]);
    requireThat(uuid(input.occurrence_id) && uuid(input.recording_id) && input.explicit_selection === true, "invalid_action");
  } else if (input.type === "remove") {
    fields(input, ["type", "occurrence_id"]);
    requireThat(uuid(input.occurrence_id), "invalid_action");
  } else if (input.type === "advance") fields(input, ["type"]);
  else if (["play", "pause", "seek"].includes(input.type)) {
    fields(input, input.type === "seek" ? ["type", "occurrence_id", "position_ms"] : ["type", "occurrence_id"]);
    requireThat(uuid(input.occurrence_id), "invalid_action");
    if (input.type === "seek") requireThat(integer(input.position_ms) && input.position_ms <= 3_600_000, "invalid_action");
  }
  else throw new RoomError("invalid_action");
}
// The headless client validates the same narrow command schema before queuing.
export { action as validateRoomAction };
function media(value) {
  fields(value, ["owner_id", "revision", "state", "duration_ms"]);
  requireThat(uuid(value.owner_id) && integer(value.revision, 1) && STATES.has(value.state)
    && integer(value.duration_ms, 10_000) && value.duration_ms <= 3_600_000, "invalid_media");
}
function fingerprint(command) {
  const a = command.action;
  if (["play", "pause", "seek"].includes(a.type)) return JSON.stringify([
    command.expected_revision, a.type, a.occurrence_id, a.position_ms ?? null]);
  // Keep the published queue-command representation stable across upgrades.
  return JSON.stringify([command.expected_revision, a.type, a.occurrence_id ?? null,
    a.recording_id ?? null, a.explicit_selection ?? null]);
}

export class RoomQueue {
  constructor(storage, { roomId, ownerId, epoch, authorize, resolveRecording, canStart = () => false, now = Date.now } = {}) {
    requireThat([roomId, ownerId, epoch].every(uuid) && typeof authorize === "function"
      && typeof resolveRecording === "function" && typeof canStart === "function" && typeof now === "function", "authority_required");
    this.storage = storage; this.sql = storage.sql;
    this.roomId = roomId; this.ownerId = ownerId; this.epoch = epoch;
    this.authorize = authorize; this.resolveRecording = resolveRecording; this.canStart = canStart; this.now = now;
    const created = now(); requireThat(integer(created) && integer(created + ROOM_LIMITS.lifetimeMs), "invalid_clock");
    storage.transactionSync(() => {
      this.sql.exec(`CREATE TABLE IF NOT EXISTS room_queue (
        singleton INTEGER PRIMARY KEY CHECK(singleton=1), room_id TEXT NOT NULL, owner_id TEXT NOT NULL,
        epoch TEXT NOT NULL, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL,
        revision INTEGER NOT NULL DEFAULT 0, current_id TEXT, pending TEXT NOT NULL DEFAULT '[]')`);
      this.sql.exec(`CREATE TABLE IF NOT EXISTS room_occurrences (
        id TEXT PRIMARY KEY, recording_id TEXT NOT NULL, added_by TEXT NOT NULL)`);
      this.sql.exec(`CREATE TABLE IF NOT EXISTS room_media (
        id TEXT PRIMARY KEY, owner_id TEXT NOT NULL, revision INTEGER NOT NULL, state TEXT NOT NULL, duration_ms INTEGER NOT NULL)`);
      this.sql.exec("CREATE TABLE IF NOT EXISTS room_actors (id TEXT PRIMARY KEY, last_command INTEGER NOT NULL)");
      this.sql.exec(`CREATE TABLE IF NOT EXISTS room_receipts (
        actor TEXT NOT NULL, command_id INTEGER NOT NULL, request TEXT NOT NULL, result TEXT NOT NULL,
        PRIMARY KEY(actor,command_id))`);
      this.sql.exec(`INSERT OR IGNORE INTO room_queue(singleton,room_id,owner_id,epoch,created_at,expires_at)
        VALUES(1,?,?,?,?,?)`, roomId, ownerId, epoch, created, created + ROOM_LIMITS.lifetimeMs);
      const row = this.room();
      requireThat(row.room_id === roomId && row.owner_id === ownerId && row.epoch === epoch, "room_identity_conflict");
      this.sql.exec(`CREATE TABLE IF NOT EXISTS room_timeline (
        singleton INTEGER PRIMARY KEY CHECK(singleton=1), anchor TEXT NOT NULL, observed_at INTEGER NOT NULL)`);
      // Existing queue stores acquire a paused anchor, never an automatic start.
      this.sql.exec("INSERT OR IGNORE INTO room_timeline(singleton,anchor,observed_at) VALUES(1,?,?)",
        JSON.stringify(selectedTimeline(this.entry(row.current_id), created)), created);
      this.sql.exec(`CREATE TABLE IF NOT EXISTS room_owner_presence (
        singleton INTEGER PRIMARY KEY CHECK(singleton=1), event_revision INTEGER NOT NULL,
        state TEXT NOT NULL, pause_at INTEGER, paused INTEGER NOT NULL)`);
      if (!this.rows("SELECT singleton FROM room_owner_presence WHERE singleton=1").length) {
        this.sql.exec("INSERT INTO room_owner_presence VALUES(1,0,'unknown',NULL,0)");
        const anchor = this.timeline();
        // A legacy playing anchor has no trusted presence history. Preserve its
        // position, but require a fresh presence decision and explicit resume.
        if (anchor.playing) {
          requireThat(created >= this.clockFloor(), "invalid_clock");
          const current = this.entry(row.current_id);
          this.storeTimeline(pausedTimeline(anchor, current, created, "owner_departed"), current);
          this.sql.exec("UPDATE room_timeline SET observed_at=? WHERE singleton=1", created);
          this.sql.exec("UPDATE room_queue SET revision=revision+1 WHERE singleton=1");
        }
      }
    });
  }
  rows(query, ...parameters) { return this.sql.exec(query, ...parameters).toArray(); }
  room() { return this.rows("SELECT * FROM room_queue WHERE singleton=1")[0]; }
  active(now = this.now()) {
    const row = this.room();
    requireThat(integer(now) && now >= row.created_at && now >= this.clockFloor(), "invalid_clock");
    requireThat(now < row.expires_at, "room_expired");
    return { ...row, observed_at: now };
  }
  clockFloor() { return this.rows("SELECT observed_at FROM room_timeline WHERE singleton=1")[0].observed_at; }
  timeline() { return JSON.parse(this.rows("SELECT anchor FROM room_timeline WHERE singleton=1")[0].anchor); }
  presence() { return this.rows("SELECT * FROM room_owner_presence WHERE singleton=1")[0]; }
  publicPresence() {
    const p = this.presence();
    return { state: p.state, pause_at: p.pause_at, paused: p.paused === 1 };
  }
  nextPresenceAlarm() { const p = this.presence(); return p.state === "disconnected" && !p.paused ? p.pause_at : null; }
  expireOwner(now) {
    const p = this.presence(), room = this.room();
    if (p.state !== "disconnected" || p.paused || now < p.pause_at || now >= room.expires_at) return false;
    const current = this.entry(room.current_id), anchor = this.timeline();
    this.storeTimeline(current ? pausedTimeline(anchor, current, p.pause_at,
      current.state === "ready" ? "owner_departed" : current.state) : selectedTimeline(null, p.pause_at), current);
    this.sql.exec("UPDATE room_owner_presence SET paused=1 WHERE singleton=1");
    this.sql.exec("UPDATE room_queue SET revision=revision+1 WHERE singleton=1");
    return true;
  }
  storeTimeline(value, current) {
    validateTimeline(value, current);
    this.sql.exec("UPDATE room_timeline SET anchor=? WHERE singleton=1", JSON.stringify(value));
  }
  member(actor, operation) {
    try { return uuid(actor) && this.authorize(actor, this.roomId, operation) === true; }
    catch { return false; }
  }
  async commit(operation) {
    try {
      const now = this.now();
      requireThat(integer(now) && now >= this.clockFloor(), "invalid_clock");
      const result = this.storage.transactionSync(() => {
        const expiredOwner = this.expireOwner(now);
        const value = operation(now, expiredOwner);
        this.sql.exec("UPDATE room_timeline SET observed_at=? WHERE singleton=1", now);
        return value;
      });
      await this.storage.sync();
      return result;
    } catch (error) {
      if (error instanceof RoomError) throw error;
      throw new RoomError("state_unavailable");
    }
  }
  cursor(actor) {
    const last = this.rows("SELECT last_command FROM room_actors WHERE id=?", actor)[0]?.last_command ?? 0;
    return { next_command_id: last + 1, receipt_floor: Math.max(1, last - ROOM_LIMITS.receipts + 1) };
  }
  entry(id) {
    if (!id) return null;
    const row = this.rows(`SELECT o.id,o.recording_id,o.added_by,m.state,m.duration_ms,m.revision AS media_revision
      FROM room_occurrences o JOIN room_media m ON o.recording_id=m.id WHERE o.id=?`, id)[0];
    return { ...row, source: "shared_local" };
  }
  async snapshot(actor) {
    return this.commit(now => {
      requireThat(this.member(actor, "read"), "unavailable");
      const row = this.active(now);
      return { room_id: row.room_id, owner_id: row.owner_id, epoch: row.epoch, revision: row.revision,
        expires_at: row.expires_at, current: this.entry(row.current_id),
        pending: JSON.parse(row.pending).map(id => this.entry(id)), self: this.cursor(actor),
        timeline: this.timeline(), owner_presence: this.publicPresence() };
    });
  }
  // Timestamp-only exchange for a future authenticated transport. Probes consume
  // no command ID; normal room maintenance may first commit a due owner pause.
  async sampleClock(actor, request) {
    fields(request, ["epoch", "probe_id"]);
    requireThat(uuid(request.epoch) && integer(request.probe_id, 1), "invalid_clock_probe");
    const sample = await this.commit(now => {
      requireThat(this.member(actor, "clock"), "unavailable");
      const room = this.active(now);
      requireThat(request.epoch === room.epoch, "stale_epoch");
      return { room_id: room.room_id, epoch: room.epoch, probe_id: request.probe_id, received_at: now };
    });
    // Include durable-storage time in server processing, and recheck expiry
    // and membership after the await. No participant grants enter the reply.
    try {
      requireThat(this.member(actor, "clock"), "unavailable");
      const room = this.active();
      requireThat(room.observed_at >= sample.received_at, "invalid_clock");
      return { ...sample, sent_at: room.observed_at };
    } catch (error) {
      if (error instanceof RoomError) throw error;
      throw new RoomError("state_unavailable");
    }
  }
  // Trusted aggregate session-authority event only. A socket's own disconnect
  // is insufficient when the owner has other authenticated sessions connected.
  async publishOwnerPresence(event) {
    fields(event, ["epoch", "revision", "connected"]);
    requireThat(uuid(event.epoch) && integer(event.revision, 1) && typeof event.connected === "boolean", "invalid_presence");
    return this.commit((now, expiredOwner) => {
      const room = this.active(now), prior = this.presence();
      requireThat(event.epoch === room.epoch, "stale_epoch");
      const state = event.connected ? "connected" : "disconnected";
      if (event.revision === prior.event_revision) requireThat(state === prior.state, "presence_revision_conflict");
      let changed = expiredOwner;
      const applied = event.revision > prior.event_revision;
      if (applied) {
        const transition = state !== prior.state;
        const pauseAt = state === "connected" ? null : prior.state === "disconnected" ? prior.pause_at
          : now + Math.min(ROOM_LIMITS.ownerGraceMs, room.expires_at - now);
        const paused = state === "disconnected" && prior.state === "disconnected" ? prior.paused : 0;
        this.sql.exec("UPDATE room_owner_presence SET event_revision=?,state=?,pause_at=?,paused=? WHERE singleton=1",
          event.revision, state, pauseAt, paused);
        if (transition) {
          this.sql.exec("UPDATE room_queue SET revision=revision+1 WHERE singleton=1"); changed = true;
        }
      }
      return { applied, changed, revision: this.room().revision, next_alarm_at: this.nextPresenceAlarm() };
    });
  }
  // A future alarm/recovery loop must call this even without participant traffic.
  async tickOwnerPresence() {
    return this.commit((now, changed) => {
      const room = this.active(now);
      return { changed, revision: room.revision, next_alarm_at: this.nextPresenceAlarm() };
    });
  }
  // command_id is a monotonically increasing integer scoped to (epoch, actor).
  // A retained receipt returns exactly the original outcome. The high-water
  // mark permanently rejects replay after the bounded receipt window expires.
  async command(actor, command) {
    fields(command, ["epoch", "command_id", "expected_revision", "action"]);
    requireThat(uuid(command.epoch) && integer(command.command_id, 1)
      && integer(command.expected_revision), "invalid_command");
    action(command.action);
    return this.commit(now => {
      requireThat(this.member(actor, "command"), "unavailable");
      const room = this.active(now);
      requireThat(command.epoch === room.epoch, "stale_epoch");
      const request = fingerprint(command);
      const prior = this.rows("SELECT * FROM room_receipts WHERE actor=? AND command_id=?", actor, command.command_id)[0];
      if (prior) {
        requireThat(prior.request === request, "command_conflict");
        return JSON.parse(prior.result);
      }
      const cursor = this.cursor(actor);
      requireThat(command.command_id >= cursor.next_command_id, "receipt_expired");
      requireThat(command.command_id === cursor.next_command_id, "command_gap");
      const known = this.rows("SELECT * FROM room_actors WHERE id=?", actor).length > 0;
      requireThat(known || this.rows("SELECT COUNT(*) AS n FROM room_actors")[0].n < ROOM_LIMITS.actors, "actor_limit");
      let failure = null;
      if (command.expected_revision !== room.revision) failure = "stale_revision";
      else {
        try { this.apply(actor, room, command.action); }
        catch (error) { if (!(error instanceof RoomError)) throw error; failure = error.code; }
      }
      const result = { epoch: room.epoch, command_id: command.command_id,
        revision: this.room().revision, accepted: failure === null, error: failure };
      this.sql.exec("INSERT INTO room_actors(id,last_command) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET last_command=excluded.last_command", actor, command.command_id);
      this.sql.exec("INSERT INTO room_receipts(actor,command_id,request,result) VALUES(?,?,?,?)", actor, command.command_id, request, JSON.stringify(result));
      this.sql.exec("DELETE FROM room_receipts WHERE actor=? AND command_id<=?", actor, command.command_id - ROOM_LIMITS.receipts);
      return result;
    });
  }
  apply(actor, room, operation) {
    if (["play", "pause", "seek"].includes(operation.type)) {
      this.applyTransport(actor, room, operation);
      this.sql.exec("UPDATE room_queue SET revision=revision+1 WHERE singleton=1");
      return;
    }
    const pending = JSON.parse(room.pending);
    let current = room.current_id;
    if (operation.type === "add") {
      requireThat(pending.length < ROOM_LIMITS.pending, "queue_full");
      requireThat(!this.rows("SELECT id FROM room_occurrences WHERE id=?", operation.occurrence_id).length, "occurrence_used");
      requireThat(this.rows("SELECT COUNT(*) AS n FROM room_occurrences")[0].n < ROOM_LIMITS.occurrences, "occurrence_limit");
      let resolved;
      try { resolved = this.resolveRecording(actor, operation.recording_id); }
      catch { throw new RoomError("recording_unavailable"); }
      requireThat(resolved !== null && resolved !== undefined, "recording_unavailable");
      media(resolved);
      requireThat(resolved.owner_id === actor, "recording_unavailable");
      const prior = this.rows("SELECT * FROM room_media WHERE id=?", operation.recording_id)[0];
      this.checkMedia(prior, resolved);
      // All rejection checks precede writes so a recorded rejection is atomic.
      this.writeMedia(operation.recording_id, prior, resolved, room.observed_at);
      this.sql.exec("INSERT INTO room_occurrences(id,recording_id,added_by) VALUES(?,?,?)", operation.occurrence_id, operation.recording_id, actor);
      pending.push(operation.occurrence_id);
    } else if (operation.type === "remove") {
      requireThat(pending.includes(operation.occurrence_id), "not_pending");
      const row = this.rows("SELECT added_by FROM room_occurrences WHERE id=?", operation.occurrence_id)[0];
      requireThat(actor === room.owner_id || row.added_by === actor, "forbidden");
      pending.splice(pending.indexOf(operation.occurrence_id), 1);
    } else {
      requireThat(actor === room.owner_id, "forbidden");
      requireThat(current !== null || pending.length > 0, "queue_empty");
      current = pending.shift() ?? null;
      this.storeTimeline(selectedTimeline(this.entry(current), room.observed_at), this.entry(current));
    }
    this.sql.exec("UPDATE room_queue SET current_id=?,pending=?,revision=revision+1 WHERE singleton=1", current, JSON.stringify(pending));
  }
  applyTransport(actor, room, operation) {
    requireThat(actor === room.owner_id, "forbidden");
    const current = this.entry(room.current_id), anchor = this.timeline(), now = room.observed_at;
    requireThat(current && current.id === operation.occurrence_id, "wrong_occurrence");
    if (operation.type === "play") {
      requireThat(this.presence().state === "connected", "owner_unavailable");
      requireThat(current.state === "ready", "recording_not_ready");
      const position = projectTimeline(anchor, current, now).position_ms;
      requireThat(position < current.duration_ms, "seek_required");
      requireThat(now + TIMELINE_LIMITS.startLeadMs < room.expires_at, "room_expiring");
      let ready = false;
      try { ready = this.canStart(Object.freeze({ epoch: room.epoch, revision: room.revision,
        occurrence_id: current.id, media_revision: current.media_revision, position_ms: position,
        server_time: now })) === true; } catch { /* Fail closed without exposing adapter data. */ }
      requireThat(ready, "readiness_required");
      requireThat(integer(now + TIMELINE_LIMITS.startLeadMs), "invalid_clock");
      // A repeated Play intent cannot delay an already scheduled/running start.
      if (!anchor.playing) this.storeTimeline({ occurrence_id: current.id, position_ms: position,
        anchor_at: now + TIMELINE_LIMITS.startLeadMs, playing: true, reason: "playing" }, current);
    } else if (operation.type === "pause") {
      this.storeTimeline(pausedTimeline(anchor, current, now,
        current.state === "ready" ? "owner_paused" : current.state), current);
    } else {
      requireThat(operation.position_ms <= current.duration_ms, "invalid_position");
      // A seek cancels the old start. Explicit Play must pass readiness again.
      this.storeTimeline({ occurrence_id: current.id, position_ms: operation.position_ms,
        anchor_at: now, playing: false, reason: current.state === "ready" ? "seeked" : current.state }, current);
    }
  }
  checkMedia(prior, next) {
    if (!prior) return;
    requireThat(prior.owner_id === next.owner_id && prior.duration_ms === next.duration_ms, "media_identity_conflict");
    if (prior.revision === next.revision) requireThat(prior.state === next.state, "media_revision_conflict");
  }
  writeMedia(id, prior, next, now) {
    if (prior && next.revision <= prior.revision) return;
    const current = this.entry(this.room().current_id);
    if (current?.recording_id === id) {
      const anchor = this.timeline(), updated = { ...current, state: next.state, media_revision: next.revision };
      if (next.state !== "ready") this.storeTimeline(pausedTimeline(anchor, current, now, next.state), updated);
      else if (current.state !== "ready") this.storeTimeline({ ...anchor, anchor_at: now, reason: "selected" }, updated);
    }
    this.sql.exec(`INSERT INTO room_media(id,owner_id,revision,state,duration_ms)
      VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,state=excluded.state`,
    id, next.owner_id, next.revision, next.state, next.duration_ms);
  }
  // Trusted storage/verification event only. Never expose as participant input.
  async publishMedia(id, next) {
    requireThat(uuid(id), "invalid_media"); media(next);
    return this.commit(now => {
      const room = this.active(now);
      const prior = this.rows("SELECT * FROM room_media WHERE id=?", id)[0];
      requireThat(prior, "recording_unavailable");
      this.checkMedia(prior, next);
      if (next.revision <= prior.revision) return { changed: false, revision: this.room().revision };
      this.writeMedia(id, prior, next, room.observed_at);
      this.sql.exec("UPDATE room_queue SET revision=revision+1 WHERE singleton=1");
      return { changed: true, revision: this.room().revision };
    });
  }
  // Synchronous adapter for the isolated upload ledger in the same authority.
  // Removed occurrences keep ownership tombstones so cancellation still works.
  allowsUpload(actor, roomId, occurrenceId, operation) {
    try {
      if (roomId !== this.roomId || !this.member(actor, "upload")) return false;
      const row = this.rows(`SELECT o.added_by,m.owner_id,m.state FROM room_occurrences o
        JOIN room_media m ON o.recording_id=m.id WHERE o.id=?`, occurrenceId)[0];
      if (row?.added_by !== actor || row.owner_id !== actor) return false;
      if (["read", "cancel"].includes(operation)) return true;
      if (!["admit", "upload"].includes(operation) || row.state !== "uploading") return false;
      const room = this.active();
      const window = [room.current_id, ...JSON.parse(room.pending)].filter(Boolean).slice(0, 3);
      return window.includes(occurrenceId);
    } catch { return false; }
  }
}

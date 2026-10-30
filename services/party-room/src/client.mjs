// In-memory protocol model only. No sockets, disk writes, UI or playback.
import { ROOM_LIMITS, validateRoomAction } from "./queue.mjs";
import { validateTimeline } from "./timeline.mjs";
import { validateReadiness } from "./readiness.mjs";

export const CLIENT_LIMITS = Object.freeze({ intents: 16 });
export const CHECKPOINT_LIMITS = Object.freeze({ version: 1, bytes: 65536 });
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const STATES = new Set(["unavailable", "uploading", "ready"]);
const TRANSPORT = new Set(["prepare", "play", "start_ready", "pause", "seek"]);
const STARTS = new Set(["play", "start_ready"]);
const REJECTIONS = new Set(["stale_revision", "queue_full", "occurrence_used", "occurrence_limit",
  "recording_unavailable", "invalid_fields", "invalid_media", "media_identity_conflict",
  "media_revision_conflict", "not_pending", "forbidden", "queue_empty", "wrong_occurrence",
  "owner_unavailable", "recording_not_ready", "seek_required", "room_expiring", "readiness_required",
  "readiness_waiting", "stale_load", "load_inactive", "participants_unavailable", "readiness_limit",
  "invalid_position", "invalid_clock"]);
const BLOCKS = new Set(["epoch_changed", "room_expired", "actor_limit", "unavailable"]);
const REVIEWS = new Set(["queue_changed", "target_changed", "occurrence_conflict", "media_changed", "load_changed"]);

export class ClientError extends Error {
  constructor(code) { super(code); this.code = code; }
}
function requireThat(value, code) { if (!value) throw new ClientError(code); }
function uuid(value) { return typeof value === "string" && UUID.test(value); }
function integer(value, minimum = 0) { return Number.isSafeInteger(value) && value >= minimum; }
function fields(value, keys) {
  requireThat(value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).sort().join(",") === [...keys].sort().join(","), "invalid_message");
}
function entry(value) {
  fields(value, ["id", "recording_id", "added_by", "state", "duration_ms", "media_revision", "source"]);
  requireThat([value.id, value.recording_id, value.added_by].every(uuid) && STATES.has(value.state)
    && integer(value.duration_ms, 10_000) && value.duration_ms <= 3_600_000
    && integer(value.media_revision, 1) && value.source === "shared_local", "invalid_message");
  return { id: value.id, recording_id: value.recording_id, added_by: value.added_by,
    state: value.state, duration_ms: value.duration_ms, media_revision: value.media_revision, source: value.source };
}
function snapshot(value) {
  fields(value, ["room_id", "owner_id", "epoch", "revision", "expires_at", "current", "pending", "self", "timeline", "owner_presence", "readiness"]);
  fields(value.self, ["next_command_id", "receipt_floor"]);
  requireThat([value.room_id, value.owner_id, value.epoch].every(uuid) && integer(value.revision)
    && integer(value.expires_at, 1) && integer(value.self.next_command_id, 1)
    && integer(value.self.receipt_floor, 1) && value.self.receipt_floor <= value.self.next_command_id
    && Array.isArray(value.pending) && value.pending.length <= ROOM_LIMITS.pending, "invalid_message");
  const current = value.current === null ? null : entry(value.current), pending = value.pending.map(entry);
  const ids = [current, ...pending].filter(Boolean).map(row => row.id);
  requireThat(new Set(ids).size === ids.length, "invalid_message");
  try { validateTimeline(value.timeline, current); } catch { throw new ClientError("invalid_message"); }
  try { validateReadiness(value.readiness, current, value.expires_at, value.timeline); } catch { throw new ClientError("invalid_message"); }
  const r = value.readiness;
  const p = value.owner_presence;
  fields(p, ["state", "pause_at", "paused"]);
  requireThat(["unknown", "connected", "disconnected"].includes(p.state) && typeof p.paused === "boolean"
    && (p.state === "disconnected" ? integer(p.pause_at, 1) && p.pause_at <= value.expires_at : p.pause_at === null && !p.paused)
    && (!(p.paused || p.state === "unknown") || !value.timeline.playing), "invalid_message");
  return { room_id: value.room_id, owner_id: value.owner_id, epoch: value.epoch,
    revision: value.revision, expires_at: value.expires_at, current, pending,
    self: { next_command_id: value.self.next_command_id, receipt_floor: value.self.receipt_floor },
    timeline: structuredClone(value.timeline), owner_presence: { state: p.state, pause_at: p.pause_at, paused: p.paused },
    readiness: { generation: r.generation, phase: r.phase, occurrence_id: r.occurrence_id,
      media_revision: r.media_revision, position_ms: r.position_ms, deadline: r.deadline, expires_at: r.expires_at,
      participant_count: r.participant_count, ready_count: r.ready_count,
      self: { included: r.self.included, status: r.self.status, next_revision: r.self.next_revision } } };
}
function receipt(value) {
  fields(value, ["epoch", "command_id", "revision", "accepted", "error"]);
  requireThat(uuid(value.epoch) && integer(value.command_id, 1) && integer(value.revision)
    && typeof value.accepted === "boolean"
    && (value.accepted ? value.error === null : REJECTIONS.has(value.error)), "invalid_message");
  return { epoch: value.epoch, command_id: value.command_id, revision: value.revision,
    accepted: value.accepted, error: value.error };
}
function queueIdentity(value) {
  const t = value.timeline;
  return JSON.stringify([value.owner_id, value.expires_at, value.current, value.pending,
    t.occurrence_id, t.position_ms, t.anchor_at, t.playing, t.reason, value.owner_presence, value.readiness]);
}
function rows(value) { return value ? [value.current, ...value.pending].filter(Boolean) : []; }
function sameRecording(left, right) {
  requireThat(left.added_by === right.added_by && left.duration_ms === right.duration_ms
    && left.source === right.source, "snapshot_conflict");
}
function consistentRows(next, previous, previews) {
  const known = [...previous, ...previews];
  const occurrences = new Map(known.map(row => [row.id, row]));
  const media = new Map(previous.map(row => [row.recording_id, row]));
  const selected = new Map(known.map(row => [row.recording_id, row]));
  const seen = new Map();
  for (const row of next) {
    const occurrence = occurrences.get(row.id), old = media.get(row.recording_id);
    if (occurrence) requireThat(occurrence.recording_id === row.recording_id, "snapshot_conflict");
    if (selected.has(row.recording_id)) sameRecording(selected.get(row.recording_id), row);
    if (old) requireThat(row.media_revision >= old.media_revision
      && (row.media_revision !== old.media_revision || row.state === old.state), "snapshot_conflict");
    const repeated = seen.get(row.recording_id);
    if (repeated) {
      sameRecording(repeated, row);
      requireThat(row.media_revision === repeated.media_revision && row.state === repeated.state, "snapshot_conflict");
    }
    seen.set(row.recording_id, row);
  }
}

function actionIdentity(value) { return JSON.stringify(Object.keys(value).sort().map(key => [key, value[key]])); }
function decodeCheckpoint(identity, encoded) {
  try {
    requireThat(typeof encoded === "string" && encoded.length <= CHECKPOINT_LIMITS.bytes
      && new TextEncoder().encode(encoded).length <= CHECKPOINT_LIMITS.bytes, "invalid_checkpoint");
    const value = JSON.parse(encoded);
    fields(value, ["version", "room_id", "actor_id", "connection_generation", "base", "intents",
      "next_intent_id", "required_next_command_id", "blocked"]);
    requireThat(value.version === CHECKPOINT_LIMITS.version && value.room_id === identity.roomId
      && value.actor_id === identity.actorId && uuid(value.room_id) && uuid(value.actor_id), "invalid_checkpoint");
    requireThat(integer(value.connection_generation) && value.connection_generation < Number.MAX_SAFE_INTEGER - 2
      && integer(value.next_intent_id, 1) && integer(value.required_next_command_id, 1)
      && (value.blocked === null || BLOCKS.has(value.blocked))
      && Array.isArray(value.intents) && value.intents.length <= CLIENT_LIMITS.intents, "invalid_checkpoint");
    const base = value.base === null ? null : snapshot(value.base);
    requireThat(base ? base.room_id === identity.roomId : value.intents.length === 0
      && value.blocked === null && value.next_intent_id === 1 && value.required_next_command_id === 1, "invalid_checkpoint");
    let previousId = 0;
    const intents = value.intents.map((item, index) => {
      fields(item, ["id", "action", "preview", "target", "from", "transport", "state", "error", "packet", "receipt"]);
      requireThat(integer(item.id, previousId + 1) && item.id < value.next_intent_id, "invalid_checkpoint");
      previousId = item.id; validateRoomAction(item.action);
      const a = item.action, preview = item.preview === null ? null : entry(item.preview);
      const target = item.target === null ? null : entry(item.target);
      requireThat(item.from === null || uuid(item.from), "invalid_checkpoint");
      if (TRANSPORT.has(a.type)) {
        fields(item.transport, ["position_ms", "playing", "load_generation"]);
        requireThat(base.owner_id === identity.actorId && preview === null && item.from === null
          && target?.id === a.occurrence_id && integer(item.transport.position_ms)
          && item.transport.position_ms <= target.duration_ms && item.transport.playing === STARTS.has(a.type)
          && integer(item.transport.load_generation) && item.transport.load_generation <= base.readiness.generation
          && (a.type !== "seek" || a.position_ms === item.transport.position_ms)
          && (a.type !== "start_ready" || a.generation === item.transport.load_generation), "invalid_checkpoint");
      } else {
        requireThat(item.transport === null, "invalid_checkpoint");
        if (a.type === "add") requireThat(preview?.id === a.occurrence_id && preview.recording_id === a.recording_id
          && preview.added_by === identity.actorId && target === null && item.from === null, "invalid_checkpoint");
        else if (a.type === "remove") requireThat(preview === null && target === null && item.from === null, "invalid_checkpoint");
        else requireThat(base.owner_id === identity.actorId && preview === null
          && (target !== null || item.from !== null) && (target === null || target.id !== item.from), "invalid_checkpoint");
      }
      requireThat(["queued", "review", "sent", "accepted", "rejected", "uncertain"].includes(item.state)
        && (index === 0 || item.state === "queued"), "invalid_checkpoint");
      if (["queued", "review"].includes(item.state)) {
        requireThat(item.packet === null && item.receipt === null
          && (item.state === "queued" ? item.error === null : REVIEWS.has(item.error)), "invalid_checkpoint");
      } else {
        fields(item.packet, ["epoch", "command_id", "expected_revision", "action"]);
        validateRoomAction(item.packet.action);
        requireThat(item.packet.epoch === base.epoch && integer(item.packet.command_id, 1)
          && item.packet.command_id < Number.MAX_SAFE_INTEGER
          && integer(item.packet.expected_revision) && item.packet.expected_revision <= base.revision
          && item.packet.command_id <= base.self.next_command_id
          && value.required_next_command_id === item.packet.command_id + 1
          && actionIdentity(item.packet.action) === actionIdentity(a), "invalid_checkpoint");
        if (["accepted", "rejected"].includes(item.state)) {
          const result = receipt(item.receipt);
          requireThat(result.epoch === item.packet.epoch && result.command_id === item.packet.command_id
            && result.accepted === (item.state === "accepted") && item.error === result.error
            && (result.accepted ? result.revision === item.packet.expected_revision + 1
              : result.revision >= item.packet.expected_revision), "invalid_checkpoint");
        } else requireThat(item.receipt === null && (item.state === "sent" ? item.error === null
          : ["receipt_expired", "command_conflict"].includes(item.error)), "invalid_checkpoint");
      }
      return { ...item, preview, target };
    });
    consistentRows(rows(base), [], intents.flatMap(item => [item.preview, item.target].filter(Boolean)));
    return { ...value, base, intents };
  } catch { throw new ClientError("invalid_checkpoint"); }
}

export class RoomClient {
  #roomId;
  #actor;
  #base = null;
  #intents = [];
  #nextIntent = 1;
  #requiredNextCommand = 1;
  #generation = 0;
  #awaiting = false;
  #connected = false;
  #blocked = null;

  constructor({ roomId, actorId }) {
    requireThat(uuid(roomId) && uuid(actorId), "invalid_identity");
    this.#roomId = roomId; this.#actor = actorId;
  }
  checkpoint() {
    const encoded = JSON.stringify({ version: CHECKPOINT_LIMITS.version, room_id: this.#roomId, actor_id: this.#actor,
      connection_generation: this.#generation, base: this.#base, intents: this.#intents, next_intent_id: this.#nextIntent,
      required_next_command_id: this.#requiredNextCommand, blocked: this.#blocked });
    decodeCheckpoint({ roomId: this.#roomId, actorId: this.#actor }, encoded);
    return encoded;
  }
  static restore(identity, encoded) {
    const value = decodeCheckpoint(identity, encoded), client = new RoomClient(identity);
    client.#base = value.base; client.#intents = value.intents; client.#nextIntent = value.next_intent_id;
    client.#requiredNextCommand = value.required_next_command_id; client.#blocked = value.blocked;
    // Persisted snapshots are display history, never a live connection. Fence
    // callbacks from the previous instance and require a fresh snapshot.
    client.#generation = value.connection_generation + 1;
    return client;
  }
  beginConnection() {
    this.#generation++; this.#connected = false; this.#awaiting = true;
    if (!["epoch_changed", "room_expired", "actor_limit"].includes(this.#blocked)) this.#blocked = null;
    return this.#generation;
  }
  disconnect(generation) {
    if (generation !== this.#generation) return;
    this.#generation++; this.#connected = false; this.#awaiting = false;
  }
  #live(generation) { return generation === this.#generation && (this.#awaiting || this.#connected); }

  receiveSnapshot(generation, input) {
    if (!this.#live(generation) || this.#blocked === "epoch_changed") return false;
    const next = snapshot(input);
    requireThat(next.room_id === this.#roomId, "wrong_room");
    if (this.#base) {
      if (next.epoch !== this.#base.epoch) {
        this.#blocked = "epoch_changed"; this.#connected = false; this.#awaiting = false;
        return false;
      }
      requireThat(next.owner_id === this.#base.owner_id && next.expires_at === this.#base.expires_at, "snapshot_conflict");
      if (next.revision < this.#base.revision || next.self.next_command_id < this.#base.self.next_command_id
        || next.self.receipt_floor < this.#base.self.receipt_floor) return false;
      if (next.revision === this.#base.revision) requireThat(queueIdentity(next) === queueIdentity(this.#base), "snapshot_conflict");
      requireThat(next.readiness.generation >= this.#base.readiness.generation, "snapshot_conflict");
      if (next.readiness.generation === this.#base.readiness.generation
        && next.readiness.self.included && this.#base.readiness.self.included) {
        requireThat(next.readiness.self.next_revision >= this.#base.readiness.self.next_revision, "snapshot_conflict");
      }
    }
    consistentRows(rows(next), rows(this.#base), this.#intents.flatMap(item => [item.preview, item.target].filter(Boolean)));
    this.#base = next; this.#connected = true; this.#awaiting = false;
    for (const intent of this.#intents) {
      if (intent.state === "sent" && intent.packet.command_id < next.self.receipt_floor) {
        intent.state = "uncertain"; intent.error = "receipt_expired";
      }
    }
    this.#retire();
    return true;
  }
  receiveReceipt(generation, input) {
    if (!this.#live(generation)) return false;
    const next = receipt(input);
    requireThat(this.#base && next.epoch === this.#base.epoch, "wrong_epoch");
    const intent = this.#intents.find(item => item.packet?.command_id === next.command_id);
    if (!intent) return false;
    requireThat(next.accepted ? next.revision === intent.packet.expected_revision + 1
      : next.revision >= intent.packet.expected_revision, "receipt_conflict");
    if (intent.receipt) requireThat(JSON.stringify(intent.receipt) === JSON.stringify(next), "receipt_conflict");
    intent.receipt = next; intent.state = next.accepted ? "accepted" : "rejected"; intent.error = next.error;
    this.#retire();
    return true;
  }
  receiveFailure(generation, commandId, code) {
    if (!this.#live(generation)) return false;
    const intent = this.#intents.find(item => item.packet?.command_id === commandId);
    if (!intent || intent.receipt) return false;
    if (["receipt_expired", "command_conflict"].includes(code)) {
      intent.state = "uncertain"; intent.error = code;
    } else if (["stale_epoch", "room_expired", "actor_limit", "unavailable"].includes(code)) {
      this.#blocked = code === "stale_epoch" ? "epoch_changed" : code;
      this.#connected = false; this.#awaiting = false;
    } else if (["command_gap", "state_unavailable"].includes(code)) {
      this.#connected = false; this.#awaiting = true;
    } else throw new ClientError("invalid_failure");
    return true;
  }
  #retire() {
    while (this.#intents[0]?.state === "accepted") {
      const intent = this.#intents[0];
      if (this.#base.revision < intent.receipt.revision || this.#base.self.next_command_id <= intent.packet.command_id) break;
      this.#intents.shift();
    }
  }

  enqueue(operation, preview) {
    requireThat(this.#base && !this.#blocked, "needs_snapshot");
    requireThat(this.#intents.length < CLIENT_LIMITS.intents, "pending_limit");
    try { validateRoomAction(operation); } catch { throw new ClientError("invalid_action"); }
    requireThat(["add", "remove", "advance"].includes(operation.type), "invalid_action");
    const shown = this.#project();
    let row = null, target = null, from = null;
    if (operation.type === "add") {
      row = entry(preview);
      requireThat(row.id === operation.occurrence_id && row.recording_id === operation.recording_id
        && row.added_by === this.#actor, "invalid_preview");
      requireThat(shown.pending.length < ROOM_LIMITS.pending
        && ![shown.current, ...shown.pending].some(item => item?.id === row.id), "occurrence_conflict");
    } else {
      requireThat(preview === undefined, "invalid_preview");
      if (operation.type === "remove") {
        const removing = shown.pending.find(item => item.id === operation.occurrence_id);
        requireThat(removing && (this.#actor === this.#base.owner_id || removing.added_by === this.#actor), "forbidden");
      } else {
        requireThat(this.#actor === this.#base.owner_id, "forbidden");
        requireThat(shown.current || shown.pending.length, "queue_empty");
        target = shown.pending[0] ?? null; from = shown.current?.id ?? null;
      }
    }
    return this.#append(operation, row, target, from, null);
  }

  // Presentation intent only. The caller supplies its displayed position;
  // this value never replaces the authority's pause/prepare/play position.
  enqueueTransport(operation, preview) {
    requireThat(this.#base && !this.#blocked, "needs_snapshot");
    requireThat(this.#intents.length < CLIENT_LIMITS.intents, "pending_limit");
    try { validateRoomAction(operation); } catch { throw new ClientError("invalid_action"); }
    requireThat(TRANSPORT.has(operation.type), "invalid_action");
    requireThat(this.#actor === this.#base.owner_id, "forbidden");
    fields(preview, ["position_ms"]);
    const current = this.#project().current;
    requireThat(current && current.id === operation.occurrence_id, "wrong_occurrence");
    requireThat(integer(preview.position_ms) && preview.position_ms <= current.duration_ms
      && (operation.type !== "seek" || preview.position_ms === operation.position_ms), "invalid_preview");
    requireThat(operation.type !== "start_ready" || operation.generation === this.#base.readiness.generation, "load_changed");
    return this.#append(operation, null, current, null, { position_ms: preview.position_ms,
      playing: STARTS.has(operation.type), load_generation: this.#base.readiness.generation });
  }

  #append(operation, preview, target, from, transport) {
    requireThat(this.#nextIntent < Number.MAX_SAFE_INTEGER, "intent_limit");
    const id = this.#nextIntent++;
    this.#intents.push({ id, action: structuredClone(operation), preview, target, from, transport,
      state: "queued", error: null, packet: null, receipt: null });
    return id;
  }

  // One outstanding command, always retried byte-for-byte. The future native
  // adapter must durably journal this packet before handing it to a socket.
  outbound() {
    if (!this.#connected || this.#blocked || !this.#intents.length) return null;
    const intent = this.#intents[0];
    if (intent.state === "sent") return structuredClone(intent.packet);
    if (intent.state !== "queued" || this.#base.self.next_command_id < this.#requiredNextCommand) return null;
    const a = intent.action;
    let conflict = null;
    if (a.type === "advance" && ((this.#base.current?.id ?? null) !== intent.from
      || (this.#base.pending[0]?.id ?? null) !== (intent.target?.id ?? null))) conflict = "queue_changed";
    if (a.type === "remove" && !this.#base.pending.some(row => row.id === a.occurrence_id)) conflict = "target_changed";
    if (a.type === "add" && (this.#base.pending.length >= ROOM_LIMITS.pending
      || [this.#base.current, ...this.#base.pending].some(row => row?.id === a.occurrence_id))) conflict = "occurrence_conflict";
    if (intent.transport) {
      if (this.#base.current?.id !== intent.target.id) conflict = "target_changed";
      else if (["prepare", ...STARTS].includes(a.type) && this.#base.current.media_revision !== intent.target.media_revision) conflict = "media_changed";
      else if (STARTS.has(a.type) && this.#base.readiness.generation !== intent.transport.load_generation) conflict = "load_changed";
    }
    if (conflict) { intent.state = "review"; intent.error = conflict; return null; }
    const id = this.#base.self.next_command_id;
    requireThat(id < Number.MAX_SAFE_INTEGER, "command_limit");
    intent.packet = { epoch: this.#base.epoch, command_id: id, expected_revision: this.#base.revision, action: structuredClone(a) };
    intent.state = "sent"; this.#requiredNextCommand = id + 1;
    return structuredClone(intent.packet);
  }
  discard(intentId) {
    const index = this.#intents.findIndex(item => item.id === intentId), intent = this.#intents[index];
    requireThat(intent && ["queued", "review", "rejected"].includes(intent.state), "outcome_unresolved");
    this.#intents.splice(index, 1);
  }
  retryRejected(intentId) {
    const intent = this.#intents.find(item => item.id === intentId);
    requireThat(intent?.state === "rejected", "outcome_unresolved");
    requireThat(this.#connected && !this.#blocked && this.#base.revision >= intent.receipt.revision
      && this.#base.self.next_command_id > intent.packet.command_id, "needs_snapshot");
    intent.packet = null; intent.receipt = null; intent.state = "queued"; intent.error = null;
  }
  // Explicitly accepting current server state is different from cancelling a
  // sent command. Only a fresh cursor proving that ID was consumed permits it.
  acceptUncertainOutcome(intentId) {
    const index = this.#intents.findIndex(item => item.id === intentId), intent = this.#intents[index];
    requireThat(intent?.state === "uncertain", "outcome_unresolved");
    requireThat(this.#connected && !this.#blocked && this.#base.self.next_command_id > intent.packet.command_id, "needs_snapshot");
    this.#intents.splice(index, 1);
  }
  #project() {
    if (!this.#base) return { current: null, pending: [] };
    let current = this.#base.current;
    let pending = [...this.#base.pending];
    const latest = new Map([current, ...pending].filter(Boolean).map(row => [row.id, row]));
    for (const intent of this.#intents) {
      const a = intent.action;
      if (intent.transport) continue;
      if (a.type === "add") {
        if (![current, ...pending].some(row => row?.id === a.occurrence_id)) pending.push(intent.preview);
      } else if (a.type === "remove") pending = pending.filter(row => row.id !== a.occurrence_id);
      else if (intent.target === null) current = null;
      else if (current?.id !== intent.target.id) {
        const index = pending.findIndex(row => row.id === intent.target.id);
        current = latest.get(intent.target.id) ?? intent.target;
        if (index >= 0) pending = pending.slice(index + 1);
      }
    }
    return { current, pending };
  }
  view() {
    const projected = this.#project(), transport = this.#intents.findLast(intent => intent.transport);
    return structuredClone({ mode: this.#blocked ? "blocked" : this.#connected ? "ready" : this.#awaiting ? "syncing" : "offline",
      blocked: this.#blocked, epoch: this.#base?.epoch ?? null, revision: this.#base?.revision ?? null,
      confirmed_timeline: this.#base?.timeline ?? null,
      confirmed_owner_presence: this.#base?.owner_presence ?? null,
      confirmed_readiness: this.#base?.readiness ?? null,
      pending_transport: transport ? { intent_id: transport.id, occurrence_id: transport.action.occurrence_id,
        action: transport.action.type, position_ms: transport.transport.position_ms, playing: transport.transport.playing,
        applies_to_current: projected.current?.id === transport.action.occurrence_id, state: transport.state, error: transport.error } : null,
      ...projected, intents: this.#intents.map(item => ({ intent_id: item.id, action: item.action,
        state: item.state, error: item.error })) });
  }
}

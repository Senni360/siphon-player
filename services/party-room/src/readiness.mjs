// Bounded startup coordination inside RoomQueue's SQLite transaction. Reports
// are participant claims, never media verification or playback authorization.
export const READINESS_LIMITS = Object.freeze({ participants: 8, waitMs: 10_000,
  lifetimeMs: 60_000, freshMs: 10_000 });
const PHASES = new Set(["idle", "collecting", "timed_out", "started", "expired"]);
const REPORTS = new Set(["ready", "buffering", "unavailable"]);
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
export class ReadinessError extends Error {
  constructor(code) { super(code); this.code = code; }
}
function requireThat(value, code = "invalid_readiness") { if (!value) throw new ReadinessError(code); }
function integer(value, min = 0) { return Number.isSafeInteger(value) && value >= min; }
function fields(value, keys) {
  requireThat(value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).sort().join(",") === [...keys].sort().join(","));
}
const idle = generation => ({ generation, phase: "idle", context: null, deadline: null, expires_at: null, members: [] });
const bindingIdentity = value => value === undefined ? undefined : value === null ? null
  : { session_id: value.session_id, output_revision: value.output_revision };

export class StartupReadiness {
  constructor(sql) {
    this.sql = sql;
    sql.exec("CREATE TABLE IF NOT EXISTS room_readiness (singleton INTEGER PRIMARY KEY CHECK(singleton=1), value TEXT NOT NULL)");
    sql.exec("INSERT OR IGNORE INTO room_readiness VALUES(1,?)", JSON.stringify(idle(0)));
  }
  read() { return JSON.parse(this.sql.exec("SELECT value FROM room_readiness WHERE singleton=1").toArray()[0].value); }
  write(value) { this.sql.exec("UPDATE room_readiness SET value=? WHERE singleton=1", JSON.stringify(value)); }
  invalidate() { const state = this.read(); if (state.phase !== "idle") this.write(idle(state.generation)); }
  prepare(context, participants, owner, now, roomExpiry, resolveOutput = () => undefined) {
    requireThat(Array.isArray(participants) && participants.length > 0 && participants.length <= READINESS_LIMITS.participants
      && participants.every(id => typeof id === "string" && UUID.test(id)) && participants.includes(owner)
      && new Set(participants).size === participants.length, "participants_unavailable");
    const generation = this.read().generation + 1;
    requireThat(integer(generation, 1), "readiness_limit");
    this.write({ generation, phase: "collecting", context, deadline: now + READINESS_LIMITS.waitMs,
      expires_at: Math.min(now + READINESS_LIMITS.lifetimeMs, roomExpiry),
      members: participants.map(id => {
        const binding = bindingIdentity(resolveOutput(id));
        return { id, revision: 0, report: null, status: binding === null ? "unavailable" : "waiting", ready_until: null, binding };
      }) });
  }
  maintain(now, member, resolveOutput = () => undefined) {
    const state = this.read(); let changed = false;
    const collecting = ["collecting", "timed_out"].includes(state.phase);
    if (!collecting && state.phase !== "started") return false;
    if (collecting && now >= state.expires_at) {
      state.phase = "expired"; changed = true;
      for (const row of state.members) { row.status = "waiting"; row.ready_until = null; }
    } else {
      if (state.phase === "collecting" && now >= state.deadline) { state.phase = "timed_out"; changed = true; }
      for (const row of state.members) {
        const binding = bindingIdentity(resolveOutput(row.id));
        if (!collecting && binding === undefined) continue;
        if (binding !== undefined && JSON.stringify(binding) !== JSON.stringify(row.binding)) {
          row.binding = binding; row.status = binding ? "waiting" : "unavailable";
          row.ready_until = null; changed = true;
        }
        const revoked = !member(row.id);
        if (revoked && row.status !== "unavailable") {
          row.status = "unavailable"; row.ready_until = null; changed = true;
        } else if (collecting && row.status === "ready" && now >= row.ready_until) {
          row.status = "waiting"; row.ready_until = null; changed = true;
        }
      }
    }
    if (changed) this.write(state);
    return changed;
  }
  report(actor, event, epoch, now, binding) {
    fields(event, ["epoch", "generation", "revision", "status"]);
    requireThat(typeof event.epoch === "string" && UUID.test(event.epoch) && integer(event.generation, 1)
      && integer(event.revision, 1) && event.revision < Number.MAX_SAFE_INTEGER && REPORTS.has(event.status));
    requireThat(event.epoch === epoch, "stale_epoch");
    const state = this.read();
    requireThat(event.generation === state.generation, "stale_load");
    requireThat(["collecting", "timed_out"].includes(state.phase), "load_inactive");
    const row = state.members.find(item => item.id === actor);
    requireThat(row, "not_in_load");
    if (binding !== undefined) requireThat(binding !== null
      && JSON.stringify(row.binding) === JSON.stringify(bindingIdentity(binding)), "stale_output");
    if (event.revision === row.revision) requireThat(event.status === row.report, "readiness_revision_conflict");
    if (event.revision <= row.revision) return false;
    row.revision = event.revision; row.report = event.status; row.status = event.status;
    row.ready_until = event.status === "ready" ? Math.min(now + READINESS_LIMITS.freshMs, state.expires_at,
      binding?.expires_at ?? state.expires_at) : null;
    this.write(state); return true;
  }
  approve(context, owner, partial, generation, startAt) {
    const state = this.read();
    requireThat(["collecting", "timed_out"].includes(state.phase)
      && JSON.stringify(state.context) === JSON.stringify(context), "readiness_required");
    if (partial) {
      requireThat(generation === state.generation, "stale_load");
      requireThat(state.phase === "timed_out", "readiness_waiting");
      requireThat(state.members.some(row => row.id === owner && row.status === "ready" && row.ready_until > startAt), "readiness_required");
    } else requireThat(state.members.every(row => row.status === "ready" && row.ready_until > startAt), "readiness_required");
  }
  started(startAt) {
    const state = this.read(); state.phase = "started";
    for (const row of state.members) {
      if (row.status === "ready" && row.ready_until <= startAt) { row.status = "waiting"; row.ready_until = null; }
    }
    this.write(state);
  }
  nextAlarm() {
    const state = this.read();
    if (!["collecting", "timed_out"].includes(state.phase)) return null;
    return Math.min(state.expires_at, ...(state.phase === "collecting" ? [state.deadline] : []),
      ...state.members.filter(row => row.status === "ready").map(row => row.ready_until));
  }
  public(actor) {
    const state = this.read(), own = state.members.find(row => row.id === actor);
    return { generation: state.generation, phase: state.phase, occurrence_id: state.context?.occurrence_id ?? null,
      media_revision: state.context?.media_revision ?? null, position_ms: state.context?.position_ms ?? null,
      deadline: state.deadline, expires_at: state.expires_at, participant_count: state.members.length,
      ready_count: state.members.filter(row => row.status === "ready").length,
      self: { included: !!own, status: own?.status ?? null, next_revision: own ? own.revision + 1 : null } };
  }
}

export function validateReadiness(value, current, roomExpiry, timeline) {
  fields(value, ["generation", "phase", "occurrence_id", "media_revision", "position_ms", "deadline", "expires_at",
    "participant_count", "ready_count", "self"]);
  fields(value.self, ["included", "status", "next_revision"]);
  requireThat(integer(value.generation) && PHASES.has(value.phase)
    && integer(value.participant_count) && value.participant_count <= READINESS_LIMITS.participants
    && integer(value.ready_count) && value.ready_count <= value.participant_count && typeof value.self.included === "boolean");
  if (value.phase === "idle") requireThat(value.occurrence_id === null && value.media_revision === null
    && value.position_ms === null && value.deadline === null && value.expires_at === null && value.participant_count === 0);
  else requireThat(value.generation > 0 && current?.id === value.occurrence_id && current.state === "ready"
    && current.media_revision === value.media_revision && integer(value.position_ms) && value.position_ms < current.duration_ms
    && integer(value.deadline, 1) && integer(value.expires_at, value.deadline) && value.expires_at <= roomExpiry
    && value.participant_count > 0 && value.position_ms === timeline.position_ms
    && timeline.playing === (value.phase === "started"));
  if (value.self.included) requireThat(value.participant_count > 0 && integer(value.self.next_revision, 1)
    && ["waiting", ...REPORTS].includes(value.self.status)
    && (value.self.status !== "ready" || value.ready_count > 0)
    && (value.self.status === "ready" || value.ready_count < value.participant_count));
  else requireThat(value.self.status === null && value.self.next_revision === null);
  if (value.phase === "expired") requireThat(value.ready_count === 0);
}

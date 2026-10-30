// Session/output coordination only. Opaque IDs are correlation fences, never
// authentication. A trusted live-connection adapter must authorize every event.
import { RoomQueue, RoomError } from "./queue.mjs";
import { pausedTimeline } from "./timeline.mjs";

export const SESSION_LIMITS = Object.freeze({ actors: 8, perActor: 2, lifetimeSessions: 64,
  leaseMs: 15_000, lifetimeMs: 15 * 60_000 });
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
function requireThat(value, code) { if (!value) throw new RoomError(code); }
function uuid(value) { return typeof value === "string" && UUID.test(value); }
function revision(value) { return Number.isSafeInteger(value) && value > 0 && value < Number.MAX_SAFE_INTEGER; }
function fields(value, keys) {
  requireThat(value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).sort().join(",") === [...keys].sort().join(","), "invalid_session");
}

export class SessionRoom extends RoomQueue {
  constructor(storage, options) {
    super(storage, options);
    this.authorizeSession = options.authorizeSession ?? (() => false);
    requireThat(typeof this.authorizeSession === "function", "authority_required");
    storage.transactionSync(() => {
      this.sql.exec("CREATE TABLE IF NOT EXISTS room_session_state (singleton INTEGER PRIMARY KEY CHECK(singleton=1))");
      this.sql.exec(`CREATE TABLE IF NOT EXISTS room_sessions (id TEXT PRIMARY KEY, actor TEXT NOT NULL,
        opened_at INTEGER NOT NULL, lease_until INTEGER NOT NULL, expires_at INTEGER NOT NULL,
        heartbeat_revision INTEGER NOT NULL, state TEXT NOT NULL, closed_at INTEGER)`);
      this.sql.exec(`CREATE TABLE IF NOT EXISTS room_outputs (actor TEXT PRIMARY KEY,
        output_revision INTEGER NOT NULL, session_id TEXT)`);
      if (!this.rows("SELECT singleton FROM room_session_state").length) {
        requireThat(this.rows("SELECT COUNT(*) AS n FROM room_sessions")[0].n === 0
          && this.rows("SELECT COUNT(*) AS n FROM room_outputs")[0].n === 0, "state_unavailable");
        const now = this.now(), room = this.active(now), anchor = this.timeline(), prior = this.presence();
        const changed = prior.state !== "unknown" || anchor.playing || this.readiness.read().phase !== "idle";
        if (anchor.playing) this.storeTimeline(pausedTimeline(anchor, this.entry(room.current_id), now, "owner_departed"), this.entry(room.current_id));
        this.readiness.invalidate();
        this.sql.exec("UPDATE room_owner_presence SET state='unknown',pause_at=NULL,paused=0 WHERE singleton=1");
        this.sql.exec("UPDATE room_timeline SET observed_at=? WHERE singleton=1", now);
        if (changed) this.sql.exec("UPDATE room_queue SET revision=revision+1 WHERE singleton=1");
        this.sql.exec("INSERT INTO room_session_state VALUES(1)");
      }
    });
    // The cohort is captured from currently live authenticated connections.
    this.readinessParticipants = () => this.rows("SELECT DISTINCT actor FROM room_sessions WHERE state='live' ORDER BY actor").map(row => row.actor);
  }
  sessionAllowed(actor, id, operation) {
    try { return uuid(actor) && this.member(actor, "session") && this.authorizeSession(actor, id, this.roomId, operation) === true; }
    catch { return false; }
  }
  session(id) { return this.rows("SELECT * FROM room_sessions WHERE id=?", id)[0]; }
  output(actor) { return this.rows("SELECT * FROM room_outputs WHERE actor=?", actor)[0]; }
  outputBinding(actor) {
    const output = this.output(actor), session = output?.session_id ? this.session(output.session_id) : null;
    return session?.state === "live" ? { session_id: session.id, output_revision: output.output_revision, expires_at: session.lease_until } : null;
  }
  readinessContext(actor, context) {
    fields(context, ["session_id", "output_revision"]);
    requireThat(uuid(context.session_id) && revision(context.output_revision), "invalid_session");
    requireThat(this.sessionAllowed(actor, context.session_id, "readiness"), "unavailable");
    const session = this.session(context.session_id);
    requireThat(session?.actor === actor && session.state === "live", "session_inactive");
    const binding = this.outputBinding(actor);
    requireThat(binding && binding.session_id === context.session_id && binding.output_revision === context.output_revision, "stale_output");
    return binding;
  }
  ownerConnected() { return this.rows("SELECT id FROM room_sessions WHERE actor=? AND state='live' LIMIT 1", this.ownerId).length > 0; }
  reconcileOwner(now) {
    const connected = this.ownerConnected(), state = connected ? "connected" : "disconnected";
    if (this.presence().state === state) return false;
    this.applyOwnerPresence({ epoch: this.epoch, revision: this.presence().event_revision + 1, connected }, now);
    return true;
  }
  beforeCommit(now) {
    if (now >= this.room().expires_at) return false;
    const wasConnected = this.ownerConnected(); let changed = false, lastOwnerLoss = null;
    for (const session of this.rows("SELECT * FROM room_sessions WHERE state='live'")) {
      const expired = now >= session.lease_until, revoked = !this.member(session.actor, "session");
      if (!expired && !revoked) continue;
      const at = expired ? session.lease_until : now;
      this.sql.exec("UPDATE room_sessions SET state=?,closed_at=? WHERE id=?", expired ? "expired" : "revoked", at, session.id);
      if (session.actor === this.ownerId) lastOwnerLoss = Math.max(lastOwnerLoss ?? 0, at);
      changed = true;
    }
    // Preserve the last lease's actual expiry even when recovery runs late.
    if (wasConnected && !this.ownerConnected()) this.reconcileOwner(lastOwnerLoss);
    return changed;
  }
  async publishOwnerPresence() { throw new RoomError("presence_managed"); }
  nextSessionAlarm() {
    const lease = this.rows("SELECT MIN(lease_until) AS at FROM room_sessions WHERE state='live'")[0].at;
    return Math.min(...[lease, this.nextPresenceAlarm(), this.readiness.nextAlarm(), this.room().expires_at].filter(value => value !== null));
  }
  validateEvent(event, extra = []) {
    fields(event, ["epoch", "session_id", ...extra]);
    requireThat(uuid(event.epoch) && uuid(event.session_id), "invalid_session");
    requireThat(event.epoch === this.epoch, "stale_epoch");
  }
  describe(actor, id) {
    const session = this.session(id), output = this.output(actor), binding = this.outputBinding(actor);
    requireThat(session?.actor === actor, "session_unavailable");
    return { session_id: id, state: session.state, lease_until: session.lease_until, expires_at: session.expires_at,
      heartbeat_revision: session.heartbeat_revision, output_revision: output.output_revision,
      is_output: binding?.session_id === id, next_output_revision: output.output_revision + 1 };
  }
  result(actor, id, applied) {
    return { applied, session: this.describe(actor, id), revision: this.room().revision, next_alarm_at: this.nextSessionAlarm() };
  }
  async openSession(actor, event) {
    this.validateEvent(event);
    return this.commit(now => {
      const room = this.active(now);
      requireThat(this.sessionAllowed(actor, event.session_id, "open"), "unavailable");
      const prior = this.session(event.session_id);
      if (prior) { requireThat(prior.actor === actor, "session_unavailable"); return this.result(actor, prior.id, false); }
      requireThat(this.rows("SELECT COUNT(*) AS n FROM room_sessions")[0].n < SESSION_LIMITS.lifetimeSessions, "session_limit");
      requireThat(this.rows("SELECT COUNT(*) AS n FROM room_sessions WHERE actor=? AND state='live'", actor)[0].n < SESSION_LIMITS.perActor, "session_limit");
      requireThat(this.output(actor) || this.rows("SELECT COUNT(*) AS n FROM room_outputs")[0].n < SESSION_LIMITS.actors, "session_actor_limit");
      const expires = Math.min(now + SESSION_LIMITS.lifetimeMs, room.expires_at);
      this.sql.exec("INSERT OR IGNORE INTO room_outputs VALUES(?,0,NULL)", actor);
      this.sql.exec("INSERT INTO room_sessions VALUES(?,?,?,?,?,0,'live',NULL)", event.session_id, actor, now,
        Math.min(now + SESSION_LIMITS.leaseMs, expires), expires);
      if (actor === this.ownerId) this.reconcileOwner(now);
      this.maintainReadiness(now);
      return this.result(actor, event.session_id, true);
    });
  }
  async heartbeatSession(actor, event) {
    this.validateEvent(event, ["revision"]); requireThat(revision(event.revision), "invalid_session");
    return this.commit(now => {
      this.active(now); requireThat(this.sessionAllowed(actor, event.session_id, "heartbeat"), "unavailable");
      const session = this.session(event.session_id);
      requireThat(session?.actor === actor && session.state === "live", "session_inactive");
      if (event.revision <= session.heartbeat_revision) return this.result(actor, session.id, false);
      this.sql.exec("UPDATE room_sessions SET heartbeat_revision=?,lease_until=? WHERE id=?", event.revision,
        Math.min(now + SESSION_LIMITS.leaseMs, session.expires_at), session.id);
      return this.result(actor, session.id, true);
    });
  }
  async closeSession(actor, event) {
    this.validateEvent(event);
    return this.commit(now => {
      this.active(now); requireThat(this.sessionAllowed(actor, event.session_id, "close"), "unavailable");
      const session = this.session(event.session_id); requireThat(session?.actor === actor, "session_unavailable");
      const applied = session.state === "live";
      if (applied) {
        this.sql.exec("UPDATE room_sessions SET state='closed',closed_at=? WHERE id=?", now, session.id);
        if (actor === this.ownerId) this.reconcileOwner(now);
        this.maintainReadiness(now);
      }
      return this.result(actor, session.id, applied);
    });
  }
  // Ordered event from the trusted output-selection adapter, not participant
  // JSON. A new revision also fences an old decoder on the same connection.
  async publishOutput(actor, event) {
    fields(event, ["epoch", "session_id", "revision"]);
    requireThat(uuid(event.epoch) && (event.session_id === null || uuid(event.session_id)) && revision(event.revision), "invalid_session");
    requireThat(event.epoch === this.epoch, "stale_epoch");
    return this.commit(now => {
      this.active(now); requireThat(this.sessionAllowed(actor, event.session_id, "output"), "unavailable");
      const prior = this.output(actor); requireThat(prior, "session_unavailable");
      if (event.revision === prior.output_revision) requireThat(event.session_id === prior.session_id, "output_revision_conflict");
      const applied = event.revision > prior.output_revision;
      if (applied) {
        if (event.session_id !== null) {
          const selected = this.session(event.session_id);
          requireThat(selected?.actor === actor && selected.state === "live", "session_inactive");
        }
        this.sql.exec("UPDATE room_outputs SET output_revision=?,session_id=? WHERE actor=?", event.revision, event.session_id, actor);
        this.maintainReadiness(now);
      }
      return { applied, output_revision: this.output(actor).output_revision, revision: this.room().revision, next_alarm_at: this.nextSessionAlarm() };
    });
  }
  async sessionStatus(actor, event) {
    this.validateEvent(event);
    return this.commit(now => {
      this.active(now); requireThat(this.sessionAllowed(actor, event.session_id, "status"), "unavailable");
      return this.result(actor, event.session_id, false);
    });
  }
  async tickSessions() {
    return this.commit((now, expiredOwner, readinessChanged, sessionsChanged) => {
      const room = this.active(now);
      return { changed: expiredOwner || readinessChanged || sessionsChanged, revision: room.revision, next_alarm_at: this.nextSessionAlarm() };
    });
  }
}

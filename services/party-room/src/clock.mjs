// Bounded in-memory estimation. No socket, timer, wall-clock mutation or audio.
export const CLOCK_LIMITS = Object.freeze({ samples: 8, minimumSamples: 3, probes: 4,
  roundTripMs: 2000, serverProcessingMs: 1000, sampleAgeMs: 30_000,
  idleGapMs: 5000, uncertaintyMs: 100, relativeDriftPpm: 200, timestampSlackMs: 2 });
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const DRIFT = CLOCK_LIMITS.relativeDriftPpm / 1_000_000;

export class ClockError extends Error {
  constructor(code) { super(code); this.code = code; }
}
function requireThat(value, code) { if (!value) throw new ClockError(code); }
function integer(value, minimum = 0) { return Number.isSafeInteger(value) && value >= minimum; }
function uuid(value) { return typeof value === "string" && UUID.test(value); }
function response(value) {
  requireThat(value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).sort().join(",") === "epoch,probe_id,received_at,room_id,sent_at", "invalid_sample");
  requireThat(uuid(value.room_id) && uuid(value.epoch) && integer(value.probe_id, 1)
    && integer(value.received_at) && integer(value.sent_at) && value.sent_at >= value.received_at, "invalid_sample");
}

export class RoomClock {
  #roomId;
  #now;
  #epoch = null;
  #generation = 0;
  #connected = false;
  #reason = "disconnected";
  #lastMono = null;
  #lastEstimate = null;
  #nextProbe = 1;
  #pending = new Map();
  #samples = [];

  constructor({ roomId, now = () => performance.now() }) {
    requireThat(uuid(roomId) && typeof now === "function", "invalid_clock_config");
    this.#roomId = roomId; this.#now = now;
  }
  beginConnection(epoch) {
    requireThat(uuid(epoch), "invalid_epoch");
    this.#invalidate("collecting"); this.#epoch = epoch; this.#generation++; this.#connected = true;
    return this.#generation;
  }
  disconnect(generation) { if (generation === this.#generation) this.#invalidate("disconnected"); }
  // Required on native suspend/resume, even when the OS monotonic clock stops.
  suspend() { this.#invalidate("suspended"); }
  #invalidate(reason) {
    this.#connected = false; this.#reason = reason; this.#lastMono = null; this.#lastEstimate = null;
    this.#pending.clear(); this.#samples = [];
  }
  #time() {
    let now;
    try { now = this.#now(); } catch { this.#invalidate("invalid_monotonic_clock"); return null; }
    if (typeof now !== "number" || !Number.isFinite(now) || now < 0 || now > Number.MAX_SAFE_INTEGER) {
      this.#invalidate("invalid_monotonic_clock"); return null;
    }
    if (this.#lastMono !== null && now < this.#lastMono) {
      this.#invalidate("monotonic_clock_regressed"); return null;
    }
    if (this.#lastMono !== null && now - this.#lastMono > CLOCK_LIMITS.idleGapMs) {
      this.#invalidate("suspend_gap"); return null;
    }
    this.#lastMono = now;
    for (const [id, sent] of this.#pending) if (now - sent > CLOCK_LIMITS.roundTripMs) this.#pending.delete(id);
    const previousCount = this.#samples.length;
    this.#samples = this.#samples.filter(sample => now - sample.at < CLOCK_LIMITS.sampleAgeMs);
    if (this.#samples.length < previousCount) this.#reason = "stale_samples";
    return now;
  }
  probe(generation) {
    if (generation !== this.#generation || !this.#connected) return null;
    const now = this.#time(); if (now === null) return null;
    requireThat(this.#pending.size < CLOCK_LIMITS.probes, "probe_limit");
    requireThat(this.#nextProbe < Number.MAX_SAFE_INTEGER, "probe_limit");
    const id = this.#nextProbe++; this.#pending.set(id, now);
    return { epoch: this.#epoch, probe_id: id };
  }
  #bounds(samples, now) {
    let lower = -Infinity, upper = Infinity;
    for (const sample of samples) {
      const drift = (now - sample.at) * DRIFT;
      lower = Math.max(lower, sample.lower - drift);
      upper = Math.min(upper, sample.upper + drift);
    }
    return { lower, upper };
  }
  accept(generation, input) {
    if (generation !== this.#generation || !this.#connected) return false;
    response(input);
    requireThat(input.room_id === this.#roomId, "wrong_room");
    const now = this.#time(); if (now === null) return false;
    const sent = this.#pending.get(input.probe_id);
    if (sent === undefined) return false;
    if (input.epoch !== this.#epoch) { this.#invalidate("epoch_changed"); return false; }
    this.#pending.delete(input.probe_id);
    const elapsed = now - sent, processing = input.sent_at - input.received_at;
    const slack = CLOCK_LIMITS.timestampSlackMs + elapsed * DRIFT;
    if (processing > CLOCK_LIMITS.serverProcessingMs || processing > elapsed + 2 * slack) return false;
    // For nonnegative one-way delays, server-minus-monotonic offset lies
    // between t2-t3 and t1-t0. Expand for timestamp resolution/clock drift.
    const sample = { lower: input.sent_at - now - slack, upper: input.received_at - sent + slack,
      at: now, rtt: Math.max(0, elapsed - processing) };
    const candidates = [...this.#samples, sample];
    const bounds = this.#bounds(candidates, now);
    if (bounds.lower > bounds.upper) { this.#invalidate("clock_discontinuity"); return false; }
    this.#samples = candidates.slice(-CLOCK_LIMITS.samples); this.#reason = "collecting";
    return true;
  }
  estimate(generation) {
    if (generation !== this.#generation) return { status: "unavailable", reason: "stale_connection" };
    if (!this.#connected) return { status: "unavailable", reason: this.#reason };
    const now = this.#time();
    if (now === null) return { status: "unavailable", reason: this.#reason };
    if (this.#samples.length < CLOCK_LIMITS.minimumSamples) return { status: "unavailable", reason: this.#reason };
    const { lower, upper } = this.#bounds(this.#samples, now);
    const lowTime = now + lower, highTime = now + upper;
    const serverTime = Math.max(this.#lastEstimate ?? 0, Math.round((lowTime + highTime) / 2));
    // A tighter sample may move the midpoint backward. Hold the previous
    // estimate and charge that entire adjustment to uncertainty instead.
    const uncertainty = Math.ceil(Math.max(serverTime - lowTime, highTime - serverTime) + 1);
    if (uncertainty > CLOCK_LIMITS.uncertaintyMs) return { status: "unavailable", reason: "uncertainty" };
    if (!integer(serverTime)) {
      this.#invalidate("clock_discontinuity"); return { status: "unavailable", reason: this.#reason };
    }
    this.#lastEstimate = serverTime;
    return { status: "ready", server_time_ms: serverTime, uncertainty_ms: uncertainty,
      sample_count: this.#samples.length, minimum_rtt_ms: Math.min(...this.#samples.map(sample => sample.rtt)) };
  }
}

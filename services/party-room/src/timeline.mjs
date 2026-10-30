// Server-time anchors only. This module does not estimate a client's clock,
// start an audio device, grant media access or choose the next queue occurrence.
export const TIMELINE_LIMITS = Object.freeze({ startLeadMs: 750 });
const REASONS = new Set(["empty", "selected", "uploading", "unavailable", "playing", "owner_paused", "owner_departed", "seeked", "preparing"]);
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

export class TimelineError extends Error {
  constructor() { super("invalid_timeline"); this.code = "invalid_timeline"; }
}
function requireThat(value) { if (!value) throw new TimelineError(); }
function integer(value) { return Number.isSafeInteger(value) && value >= 0; }

export function validateTimeline(value, current) {
  requireThat(current === null || (current && typeof current === "object" && UUID.test(current.id)
    && integer(current.duration_ms) && current.duration_ms >= 10_000 && current.duration_ms <= 3_600_000
    && ["ready", "uploading", "unavailable"].includes(current.state)));
  requireThat(value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).sort().join(",") === "anchor_at,occurrence_id,playing,position_ms,reason");
  requireThat(integer(value.position_ms) && integer(value.anchor_at) && typeof value.playing === "boolean"
    && REASONS.has(value.reason) && value.occurrence_id === (current?.id ?? null));
  if (!current) requireThat(value.position_ms === 0 && !value.playing && value.reason === "empty");
  else {
    requireThat(integer(current.duration_ms) && value.position_ms <= current.duration_ms && value.reason !== "empty");
    requireThat(value.playing === (value.reason === "playing"));
    if (value.playing) requireThat(current.state === "ready" && value.position_ms < current.duration_ms);
    if (current.state !== "ready") requireThat(!value.playing && value.reason === current.state);
    else requireThat(!["uploading", "unavailable"].includes(value.reason));
  }
}

export function selectedTimeline(current, now) {
  const value = { occurrence_id: current?.id ?? null, position_ms: 0, anchor_at: now,
    playing: false, reason: current ? current.state === "ready" ? "selected" : current.state : "empty" };
  validateTimeline(value, current);
  return value;
}

// The caller supplies a validated estimate of server time. A paused anchor never
// advances, future starts hold their position, and EOF never advances the queue.
export function projectTimeline(value, current, serverNow) {
  validateTimeline(value, current); requireThat(integer(serverNow));
  const position = Math.min(current?.duration_ms ?? 0, value.position_ms
    + (value.playing ? Math.max(0, serverNow - value.anchor_at) : 0));
  const ended = current !== null && position === current.duration_ms;
  return { occurrence_id: value.occurrence_id, position_ms: position,
    playing: value.playing && serverNow >= value.anchor_at && !ended,
    scheduled: value.playing && serverNow < value.anchor_at, ended };
}

export function pausedTimeline(value, current, now, reason) {
  return { occurrence_id: current.id, position_ms: projectTimeline(value, current, now).position_ms,
    anchor_at: now, playing: false, reason };
}

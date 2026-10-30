// Timeline/presence fixtures model a ready solo decoder. Readiness-specific
// tests send reports explicitly and never use this convenience helper.
import { READINESS_LIMITS } from "../src/readiness.mjs";
import { TIMELINE_LIMITS } from "../src/timeline.mjs";

export async function prepareSolo(queue, owner, action, now) {
  if (action.type !== "play") return;
  const view = await queue.snapshot(owner);
  if (view.timeline.playing || view.owner_presence.state !== "connected" || view.current?.state !== "ready"
    || view.current.id !== action.occurrence_id || view.timeline.position_ms >= view.current.duration_ms
    || now + READINESS_LIMITS.waitMs + TIMELINE_LIMITS.startLeadMs >= view.expires_at) return;
  const result = await queue.command(owner, { epoch: view.epoch, command_id: view.self.next_command_id,
    expected_revision: view.revision, action: { type: "prepare", occurrence_id: action.occurrence_id } });
  if (!result.accepted) throw new Error(`Fixture prepare failed: ${result.error}`);
  const prepared = await queue.snapshot(owner);
  await queue.publishReadiness(owner, { epoch: view.epoch, generation: prepared.readiness.generation,
    revision: 1, status: "ready" });
}

// Local test fixture only. Fixed fake identities are not authentication.
import { RoomQueue, ROOM_LIMITS } from "../../src/queue.mjs";
import { projectTimeline, TIMELINE_LIMITS } from "../../src/timeline.mjs";
import { READINESS_LIMITS } from "../../src/readiness.mjs";
import { UploadLedger } from "../../../media-gateway/src/uploads.mjs";

const uuid = n => `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
const owner = uuid(1), member = uuid(2), roomId = uuid(3), epoch = uuid(4);
const occurrence = n => uuid(100 + n), recording = uuid(200);
const add = n => ({ type: "add", occurrence_id: occurrence(n), recording_id: recording, explicit_selection: true });
function check(value, reason) { if (!value) throw new Error(`Room runtime assertion: ${reason}`); }

export class QueueProbe {
  constructor(state) { this.storage = state.storage; }
  async fetch() {
    let now = 1_800_000_000_000, ready = false;
    const options = { roomId, ownerId: owner, epoch, now: () => now, canStart: () => ready,
      readinessParticipants: () => [owner, member],
      authorize: actor => [owner, member].includes(actor),
      resolveRecording: actor => actor === owner
        ? { owner_id: owner, revision: 1, state: "uploading", duration_ms: 12000 } : null };
    let queue = new RoomQueue(this.storage, options);
    const send = async (actor, action) => {
      const snapshot = await queue.snapshot(actor);
      return queue.command(actor, { epoch, command_id: snapshot.self.next_command_id,
        expected_revision: snapshot.revision, action });
    };
    const report = async actor => {
      const view = await queue.snapshot(actor);
      return queue.publishReadiness(actor, { epoch, generation: view.readiness.generation,
        revision: view.readiness.self.next_revision, status: "ready" });
    };
    const prepare = async () => {
      const view = await queue.snapshot(owner);
      check((await send(owner, { type: "prepare", occurrence_id: view.current.id })).accepted, "prepare accepted");
    };
    for (let n = 1; n <= 4; n++) check((await send(owner, add(n))).accepted, "add occurrence");
    const advance = { epoch, command_id: 5, expected_revision: 4, action: { type: "advance" } };
    const receipt = await queue.command(owner, advance);
    queue = new RoomQueue(this.storage, options);
    check(JSON.stringify(await queue.command(owner, advance)) === JSON.stringify(receipt), "original receipt survives reconstruction");
    check((await queue.snapshot(owner)).current.id === occurrence(1), "advance was not repeated");
    check((await send(member, { type: "remove", occurrence_id: occurrence(2) })).error === "forbidden", "member ownership enforced");
    const stale = await queue.command(member, { epoch, command_id: 2, expected_revision: 0, action: { type: "advance" } });
    check(!stale.accepted && stale.error === "stale_revision", "old revisions cannot mutate");

    const uploads = new UploadLedger(this.storage, { now: options.now, authorize: (...args) => queue.allowsUpload(...args) });
    const selection = n => ({ id: uuid(300 + n), room_id: roomId, occurrence_id: occurrence(n), size: 1,
      sha256: "a".repeat(64), profile: "mp3-44100-stereo", explicit_selection: true });
    let refused = false;
    try { await uploads.admit(owner, selection(4)); } catch (error) { refused = error.code === "unavailable"; }
    check(refused, "fourth occurrence not upload eligible");
    await send(owner, { type: "advance" });
    check((await uploads.admit(owner, selection(4))).state === "reserved", "window moves with queue");
    await send(owner, { type: "remove", occurrence_id: occurrence(4) });
    check((await uploads.cancel(owner, selection(4).id, 1)).state === "cancelled", "removed row stays cancellable");

    await queue.publishMedia(recording, { owner_id: owner, revision: 3, state: "ready", duration_ms: 12000 });
    await queue.publishMedia(recording, { owner_id: owner, revision: 2, state: "unavailable", duration_ms: 12000 });
    let final = await queue.snapshot(member);
    check(final.current.id === occurrence(2) && final.current.state === "ready", "stale media event ignored");
    check(final.pending.length === 1 && final.pending[0].id === occurrence(3), "only intended occurrence removed");
    await queue.publishOwnerPresence({ epoch, revision: 1, connected: true });
    check((await send(owner, { type: "play", occurrence_id: occurrence(2) })).error === "readiness_required", "start gate fails closed");
    await prepare(); await report(owner); await report(member);
    check((await send(owner, { type: "play", occurrence_id: occurrence(2) })).error === "readiness_required", "ready votes cannot bypass access gate");
    ready = true;
    const startView = await queue.snapshot(owner);
    const play = { epoch, command_id: startView.self.next_command_id, expected_revision: startView.revision,
      action: { type: "play", occurrence_id: occurrence(2) } };
    const started = await queue.command(owner, play);
    check(started.accepted, "ready start accepted");
    now += TIMELINE_LIMITS.startLeadMs + 1500;
    queue = new RoomQueue(this.storage, options);
    check(JSON.stringify(await queue.command(owner, play)) === JSON.stringify(started), "play receipt survives reconstruction");
    final = await queue.snapshot(owner);
    check(projectTimeline(final.timeline, final.current, now).position_ms === 1500, "original start time survives reconstruction");
    await send(owner, { type: "pause", occurrence_id: occurrence(2) });
    now += 5000;
    final = await queue.snapshot(owner);
    check(projectTimeline(final.timeline, final.current, now).position_ms === 1500, "pause freezes position");
    await queue.publishMedia(recording, { owner_id: owner, revision: 4, state: "unavailable", duration_ms: 12000 });
    final = await queue.snapshot(owner);
    check(!final.timeline.playing && final.timeline.reason === "unavailable", "media loss pauses anchor");
    await queue.publishMedia(recording, { owner_id: owner, revision: 5, state: "ready", duration_ms: 12000 });
    final = await queue.snapshot(owner);
    check(!final.timeline.playing && final.timeline.position_ms === 1500, "ready never resumes on its own");
    const sample = await queue.sampleClock(owner, { epoch, probe_id: 7 });
    check(sample.room_id === roomId && sample.epoch === epoch && sample.probe_id === 7, "clock sample is room scoped");
    check(sample.received_at === now && sample.sent_at === now, "clock sample uses injected authority time");
    check(JSON.stringify(await queue.snapshot(owner)) === JSON.stringify(final), "clock sample leaves queue/revision/cursor unchanged");
    await prepare(); await report(owner); await report(member);
    check((await send(owner, { type: "play", occurrence_id: occurrence(2) })).accepted, "fresh resume accepted");
    await queue.publishOwnerPresence({ epoch, revision: 2, connected: false });
    now += ROOM_LIMITS.ownerGraceMs + 1000;
    queue = new RoomQueue(this.storage, options);
    check((await queue.tickOwnerPresence()).changed, "overdue owner departure survives reconstruction");
    check(!(await queue.tickOwnerPresence()).changed, "departure applies once");
    await queue.publishOwnerPresence({ epoch, revision: 3, connected: true });
    final = await queue.snapshot(owner);
    check(!final.timeline.playing && final.timeline.reason === "owner_departed", "owner return never auto-resumes");
    await send(owner, { type: "seek", occurrence_id: occurrence(2), position_ms: 1000 });
    await prepare(); await report(owner);
    const preparing = await queue.snapshot(owner);
    now += READINESS_LIMITS.waitMs;
    queue = new RoomQueue(this.storage, options);
    check((await queue.tickReadiness()).changed, "load timeout survives reconstruction");
    final = await queue.snapshot(owner);
    check(final.readiness.phase === "timed_out" && !final.timeline.playing, "timeout does not auto-start");
    const duplicate = await queue.publishReadiness(owner, { epoch, generation: preparing.readiness.generation, revision: 1, status: "ready" });
    check(!duplicate.applied && (await queue.snapshot(owner)).readiness.ready_count === 0, "duplicate cannot revive expired vote");
    await report(owner);
    const partial = await send(owner, { type: "start_ready", occurrence_id: occurrence(2), generation: preparing.readiness.generation });
    check(partial.accepted && (await queue.snapshot(member)).readiness.self.status === "waiting", "explicit partial start leaves peer waiting");
    await send(owner, { type: "seek", occurrence_id: occurrence(2), position_ms: 2000 });
    await prepare();
    let staleLoad = false;
    try { await queue.publishReadiness(member, { epoch, generation: preparing.readiness.generation, revision: 1, status: "ready" }); }
    catch (error) { staleLoad = error.code === "stale_load"; }
    check(staleLoad, "previous load callback refused after seek and new preparation");
    final = await queue.snapshot(owner);
    return Response.json({ cases: 12, revision: final.revision, current: final.current.id, pending: final.pending.length,
      generation: final.readiness.generation, playing: final.timeline.playing });
  }
}

export default {
  fetch(request, env) {
    if (new URL(request.url).hostname !== "localhost") return new Response(null, { status: 404 });
    return env.QUEUE_TEST.get(env.QUEUE_TEST.newUniqueId()).fetch("http://test/run");
  },
};

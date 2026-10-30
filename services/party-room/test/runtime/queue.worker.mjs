// Local test fixture only. Fixed fake identities are not authentication.
import { RoomQueue } from "../../src/queue.mjs";
import { UploadLedger } from "../../../media-gateway/src/uploads.mjs";

const uuid = n => `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
const owner = uuid(1), member = uuid(2), roomId = uuid(3), epoch = uuid(4);
const occurrence = n => uuid(100 + n), recording = uuid(200);
const add = n => ({ type: "add", occurrence_id: occurrence(n), recording_id: recording, explicit_selection: true });
function check(value, reason) { if (!value) throw new Error(`Room runtime assertion: ${reason}`); }

export class QueueProbe {
  constructor(state) { this.storage = state.storage; }
  async fetch() {
    const options = { roomId, ownerId: owner, epoch, now: () => 1_800_000_000_000,
      authorize: actor => [owner, member].includes(actor),
      resolveRecording: actor => actor === owner
        ? { owner_id: owner, revision: 1, state: "uploading", duration_ms: 12000 } : null };
    let queue = new RoomQueue(this.storage, options);
    const send = async (actor, action) => {
      const snapshot = await queue.snapshot(actor);
      return queue.command(actor, { epoch, command_id: snapshot.self.next_command_id,
        expected_revision: snapshot.revision, action });
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
    const final = await queue.snapshot(member);
    check(final.current.id === occurrence(2) && final.current.state === "ready", "stale media event ignored");
    check(final.pending.length === 1 && final.pending[0].id === occurrence(3), "only intended occurrence removed");
    return Response.json({ cases: 4, revision: final.revision, current: final.current.id, pending: final.pending.length });
  }
}

export default {
  fetch(request, env) {
    if (new URL(request.url).hostname !== "localhost") return new Response(null, { status: 404 });
    return env.QUEUE_TEST.get(env.QUEUE_TEST.newUniqueId()).fetch("http://test/run");
  },
};

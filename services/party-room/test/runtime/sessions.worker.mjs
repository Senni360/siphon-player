// Local-only fixture. Fixed connection proofs are not production authentication.
import { SessionRoom, SESSION_LIMITS } from "../../src/sessions.mjs";
import { ROOM_LIMITS } from "../../src/queue.mjs";

const uuid = n => `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
const owner = uuid(1), member = uuid(2), roomId = uuid(3), epoch = uuid(4);
const occurrence = uuid(5), recording = uuid(6), oldSession = uuid(10), peerSession = uuid(11), newSession = uuid(12);
const event = (session_id, revision) => revision === undefined ? { epoch, session_id } : { epoch, session_id, revision };
function check(value, reason) { if (!value) throw new Error(`Session runtime assertion: ${reason}`); }

export class SessionProbe {
  constructor(state) { this.storage = state.storage; }
  async fetch() {
    let now = 1_800_000_000_000;
    const options = { roomId, ownerId: owner, epoch, now: () => now, canStart: () => true,
      authorize: actor => [owner, member].includes(actor),
      authorizeSession: (actor, id) => actor === owner ? [oldSession, newSession].includes(id) : id === peerSession,
      resolveRecording: () => ({ owner_id: owner, revision: 1, state: "ready", duration_ms: 120000 }) };
    let room = new SessionRoom(this.storage, options);
    const send = async type => {
      const view = await room.snapshot(owner);
      return room.command(owner, { epoch, command_id: view.self.next_command_id, expected_revision: view.revision,
        action: type === "add" ? { type, occurrence_id: occurrence, recording_id: recording, explicit_selection: true }
          : type === "advance" ? { type } : { type, occurrence_id: occurrence } });
    };
    const report = async (actor, session_id, output_revision, revision) => {
      const view = await room.snapshot(actor);
      return room.publishReadiness(actor, { epoch, generation: view.readiness.generation,
        revision: revision ?? view.readiness.self.next_revision, status: "ready" }, { session_id, output_revision });
    };
    const first = await room.openSession(owner, event(oldSession));
    await room.openSession(member, event(peerSession));
    await room.publishOutput(owner, event(oldSession, 1)); await room.publishOutput(member, event(peerSession, 1));
    check((await room.snapshot(owner)).self.next_command_id === 1, "connection events do not consume command IDs");
    now += 1000;
    check((await room.openSession(owner, event(oldSession))).session.lease_until === first.session.lease_until, "retry never renews lease");
    await send("add"); await send("advance"); await send("prepare");
    await report(owner, oldSession, 1); await report(member, peerSession, 1);
    const second = await room.openSession(owner, event(newSession));
    await room.publishOutput(owner, event(newSession, 2));
    let denied = false;
    try { await report(owner, oldSession, 1); } catch (error) { denied = error.code === "stale_output"; }
    check(denied && (await room.snapshot(owner)).readiness.self.status === "waiting", "old output cannot make replacement ready");
    check(!(await report(owner, newSession, 2, 1)).applied, "old report cursor cannot revive readiness");
    await report(owner, newSession, 2);
    const view = await room.snapshot(owner), packet = { epoch, command_id: view.self.next_command_id,
      expected_revision: view.revision, action: { type: "play", occurrence_id: occurrence } };
    const receipt = await room.command(owner, packet); check(receipt.accepted, "fresh selected output starts");
    const anchor = (await room.snapshot(owner)).timeline;
    room = new SessionRoom(this.storage, options);
    check(JSON.stringify(await room.command(owner, packet)) === JSON.stringify(receipt), "reconstruction preserves command receipt");
    check((await room.sessionStatus(owner, event(newSession))).session.lease_until === second.session.lease_until, "reconstruction preserves session lease");
    await room.closeSession(owner, event(oldSession));
    check((await room.snapshot(owner)).owner_presence.state === "connected", "closing old owner connection preserves aggregate presence");
    now += SESSION_LIMITS.leaseMs + ROOM_LIMITS.ownerGraceMs + 4000;
    room = new SessionRoom(this.storage, options); await room.tickSessions();
    const paused = await room.snapshot(owner), deadline = second.session.lease_until + ROOM_LIMITS.ownerGraceMs;
    check(!paused.timeline.playing && paused.timeline.anchor_at === deadline
      && paused.timeline.position_ms === deadline - anchor.anchor_at, "late maintenance pauses at original lease-plus-grace deadline");
    check(!(await room.openSession(owner, event(newSession))).applied, "expired session ID cannot be reopened");
    check(!(await room.snapshot(owner)).timeline.playing, "expired session retry never resumes playback");
    const encoded = JSON.stringify(paused);
    check(![oldSession, newSession, peerSession, "session_id", "output_revision"].some(value => encoded.includes(value)), "snapshots hide connection identities");
    return Response.json({ cases: 8, paused: true, lease_preserved: true });
  }
}

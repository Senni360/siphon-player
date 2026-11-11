// Telling the computer to do something, and asking it something.

import { uuid, toast } from './dom.js';
import { answered } from './errors.js';
import { edgeAsk, edgeSend } from './edge.js';
import { EDGE_DIRECT } from './edge-library.js';
import { api } from './service.js';
import { DIRECT, DIRECT_ACTS, hasSpotify } from './spotify.js';
import { computerLooked, events, here, hooks, openedAt, player, readyReplies, replyWaiters, WAKE_MS } from './state.js';

/** How long the first screen waits to learn whether the computer is there. */
const LOOK_MS = 1500;

/** Tells the computer to do something, or Spotify when the computer's not there. */
export async function act(kind, args = {}) {
  player.lastAct = Date.now();
  // Playing on this phone, the buttons work the phone's own player.
  if (here.active && hooks.hereActs[kind]) return hooks.hereActs[kind](args);
  if (!player.online && hasSpotify() && DIRECT_ACTS[kind]) {
    try {
      await DIRECT_ACTS[kind](args);
      setTimeout(() => hooks.pollSpotify(), 500);
    } catch (error) { toast(error.message); }
    return;
  }
  // A computer on the edge hears this at once. Otherwise the relay carries it.
  if (edgeSend({ t: 'command', id: uuid(), kind, ...args })) return;
  await api('/live/remote/send', 'POST', { id: uuid(), kind, ...args });
}

/** Tells the computer itself to do something, whichever device is playing. */
export async function tell(kind, args = {}) {
  if (edgeSend({ t: 'command', id: uuid(), kind, ...args })) return;
  await api('/live/remote/send', 'POST', { id: uuid(), kind, ...args });
}

/** Asks the computer something, and waits for it to answer. */
export async function ask(kind, args = {}, patience = 45000) {
  // The edge reads the library itself, so a cold open fills the screen at once
  // instead of waiting out a computer that is off. A computer that is there
  // keeps answering: the first screen waits just long enough to know which,
  // and only the edge being unreachable puts the page back on the old path.
  if (!player.online && EDGE_DIRECT[kind]) {
    await Promise.race([computerLooked(), new Promise(resolve => setTimeout(resolve, LOOK_MS))]);
    if (!player.online) {
      const found = await EDGE_DIRECT[kind](args);
      if (found !== null) return found;
    }
  }
  if (!player.online && hasSpotify() && DIRECT[kind]) return DIRECT[kind](args);
  // Past the time the computer takes to notice, it isn't coming: say so straight away.
  if (!player.online && Date.now() - openedAt > WAKE_MS) throw new Error("Encore isn't open on your computer.");
  // A computer on the edge answers there, in one round trip. An answer too
  // large for its socket comes back as a small error; the relay carries it.
  const overEdge = edgeAsk(kind, args, patience);
  if (overEdge) {
    try { return await overEdge; }
    catch (error) { if (error?.message !== 'answer_too_large') throw error; }
  }
  const id = uuid();
  await api('/live/remote/send', 'POST', { id, kind, ...args });
  const until = Date.now() + patience;
  try {
    while (Date.now() < until) {
      // The event stream says the moment it's ready; without it, look every so often.
      if (!readyReplies.delete(id)) {
        await new Promise(resolve => {
          const timer = setTimeout(resolve, events.open ? 2000 : 600);
          replyWaiters.set(id, () => { clearTimeout(timer); resolve(); });
        });
      }
      const answer = await api(`/live/remote/reply/${id}`);
      if (answer.ready) {
        // The computer itself answered: no retry changes this.
        if (answer.reply.error) throw answered(answer.reply.error);
        return answer.reply;
      }
    }
  } finally { replyWaiters.delete(id); }
  throw new Error(player.online ? 'Encore took too long to answer.' : "Encore isn't open on your computer.");
}

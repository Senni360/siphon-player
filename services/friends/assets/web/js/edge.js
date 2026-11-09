// The always-on edge.
//
// The phone asks this group service for short-lived edge tokens, then speaks to
// the edge directly: audio there, and state and commands over one WebSocket. All
// of it is optional. The edge can be absent, slow or refuse a song, and the
// computer still answers exactly as it does today. Nothing here ever shows an
// error of its own.
//
// The player's state machine and its own controls are handed in with
// `setEdgeSinks`, so this module never imports upwards into it.

import { STORE } from './auth.js';
import { answered } from './errors.js';
import { uuid } from './dom.js';
import { api } from './service.js';

const EDGE_REFRESH_MS = 4 * 60 * 1000;
const EDGE_TIMEOUT_MS = 8000;
/** A computer counts as on the socket after this much silence: no. */
const EDGE_PEER_MS = 45000;
/** Reconnects wait 1s, 2s, 4s, then every 8s. */
const EDGE_RETRY_MS = [1000, 2000, 4000, 8000];
export const edge = {
  tokens: null, at: 0, asking: null, restUntil: 0,
  socket: null, ready: false, opened: false, peerAt: 0, seq: 0, stateAt: 0,
  retry: 0, timer: null, waits: new Map(),
};

/** The player takes the edge's state and its commands: no import back into it. */
const sinks = { state: null, control: null };
export function setEdgeSinks(state, control) {
  sinks.state = state;
  sinks.control = control;
}

/** The phone's own edge tokens, fetched lazily and refreshed before they run out. */
export async function edgeTokens(fresh = false) {
  if (!fresh && edge.tokens && Date.now() - edge.at < EDGE_REFRESH_MS) return edge.tokens;
  if (!fresh && Date.now() < edge.restUntil) return null;
  if (edge.asking) return edge.asking;
  edge.asking = api('/edge/token', 'POST', { role: 'phone' })
    .then(answer => {
      if (typeof answer?.url !== 'string' || !/^https?:\/\//.test(answer.url)
        || typeof answer.live !== 'string' || typeof answer.media !== 'string' || typeof answer.sync !== 'string') throw new Error('no edge');
      edge.tokens = { url: answer.url.replace(/\/+$/, ''), live: answer.live, media: answer.media, sync: answer.sync };
      edge.at = Date.now();
      edge.restUntil = 0;
      return edge.tokens;
    })
    .catch(() => {
      // A service without the edge is asked rarely, not once per song.
      edge.tokens = null;
      edge.restUntil = Date.now() + 60000;
      return null;
    })
    .finally(() => { edge.asking = null; });
  return edge.asking;
}

/** One edge call. A token that ran out is replaced once, then never retried. */
export async function edgeCall(path, scope, { method = 'POST', body, timeout = EDGE_TIMEOUT_MS } = {}) {
  for (let attempt = 0; attempt < 2; attempt++) {
    const tokens = await edgeTokens(attempt > 0);
    if (!tokens) return null;
    let answer;
    try {
      answer = await fetch(`${tokens.url}${path}`, {
        method,
        headers: {
          ...(body !== undefined ? { 'content-type': 'application/json' } : {}),
          authorization: `Bearer ${tokens[scope]}`,
          accept: 'application/json',
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        cache: 'no-store',
        signal: AbortSignal.timeout(timeout),
      });
    } catch { return null; }
    if (answer.status === 401 && attempt === 0) continue;
    if (!answer.ok) return null;
    return await answer.json().catch(() => null);
  }
  return null;
}

/** Where the edge says a source's audio is, as this page can fetch it. */
export async function edgeResolve(source) {
  if (!source) return null;
  const found = await edgeCall('/v1/resolve', 'media', { body: { source } });
  if (!found || typeof found.url !== 'string' || !edge.tokens) return null;
  const { url } = found;
  const full = /^https?:\/\//.test(url) ? url : `${edge.tokens.url}${url.startsWith('/') ? '' : '/'}${url}`;
  return { url: full, hls: false };
}

/** The public SoundCloud source the edge accepts, when the song names one. */
const SOUNDCLOUD_SOURCE = /^soundcloud:[A-Za-z0-9][A-Za-z0-9._-]*(?:\/[A-Za-z0-9][A-Za-z0-9._-]*){1,4}$/;
export function edgeSoundcloudSource(song) {
  const uri = String(song?.uri ?? '');
  if (SOUNDCLOUD_SOURCE.test(uri)) return uri;
  // Encore's own rows carry the track's permalink. A private link has a query
  // string and is never sent to the edge.
  const link = song?.link;
  if (typeof link !== 'string' || !link.startsWith('https://')) return null;
  try {
    const url = new URL(link);
    if (!['soundcloud.com', 'www.soundcloud.com', 'm.soundcloud.com'].includes(url.hostname)) return null;
    if (url.search || url.hash || !/^\/[A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*\/?$/.test(url.pathname)) return null;
    return `soundcloud:${url.pathname.replace(/^\/|\/$/g, '')}`;
  } catch { return null; }
}

/** Saved Spotify matches, read at most once a minute. */
let savedMatches = { at: 0, data: null };
export async function edgeSavedMatch(uri) {
  if (!savedMatches.data || Date.now() - savedMatches.at > 60000) {
    const doc = await edgeCall('/v1/sync/matches', 'sync', { method: 'GET' });
    if (doc) savedMatches = { at: Date.now(), data: doc.data && typeof doc.data === 'object' ? doc.data : {} };
  }
  if (!savedMatches.data) return null;
  const entry = savedMatches.data[uri];
  const source = typeof entry === 'string' ? entry : entry?.sourceUri ?? entry?.source;
  return typeof source === 'string' && source ? source : null;
}

/** A Spotify song as the same song on YouTube or SoundCloud, then that audio. */
export async function edgeSpotifyStream(song) {
  const saved = await edgeSavedMatch(song.uri);
  if (saved) {
    const found = await edgeResolve(saved);
    if (found) return found;
  }
  const found = await edgeCall('/v1/match', 'media', {
    body: {
      uri: song.uri, name: song.name,
      durationMs: Math.round(song.duration_ms ?? 0),
      artists: (song.artists ?? []).map(name => ({ name })),
    },
    timeout: 12000,
  });
  const source = found?.match?.source;
  if (typeof source !== 'string' || !source) return null;
  return await edgeResolve(source);
}

/** The sources of the next songs, so the edge starts fetching them now. */
export async function edgeWarm(songs) {
  const sources = [];
  for (const song of songs) {
    const uri = String(song?.uri ?? '');
    if (/^youtube:video:[A-Za-z0-9_-]{11}$/.test(uri)) sources.push(uri);
    else if (uri.startsWith('soundcloud:')) {
      const source = edgeSoundcloudSource(song);
      if (source) sources.push(source);
    } else if (uri.startsWith('spotify:')) {
      const saved = await edgeSavedMatch(uri);
      if (saved) sources.push(saved);
    }
    if (sources.length === 2) break;
  }
  if (sources.length) await edgeCall('/v1/warm', 'media', { body: { sources } });
}

/** Whether a computer is on the edge socket, hearing this phone at once. */
export const edgePeer = () => edge.ready && Date.now() - edge.peerAt < EDGE_PEER_MS;
export function edgeSend(message) {
  if (!edgePeer()) return false;
  const socket = edge.socket;
  if (!socket || socket.readyState !== WebSocket.OPEN) return false;
  try { socket.send(JSON.stringify({ v: 1, ...message })); return true; } catch { return false; }
}

/** A command over the socket, answered by the computer on it. */
export function edgeAsk(kind, args, patience) {
  const id = uuid();
  if (!edgeSend({ t: 'command', id, kind, ...args })) return null;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      edge.waits.delete(id);
      reject(new Error('Encore took too long to answer.'));
    }, patience);
    edge.waits.set(id, { resolve, reject, timer });
  });
}

function edgeApply(body) {
  if (!body || typeof body !== 'object') return;
  const state = body.state && typeof body.state === 'object' ? body.state : body;
  if (!state || typeof state !== 'object' || !('song' in state) && !('position_ms' in state)) return;
  const now = Date.now();
  if (typeof state.at === 'number' && state.at > edge.stateAt) edge.stateAt = state.at;
  Promise.resolve(sinks.state?.({ ...state, at: typeof state.at === 'number' ? state.at : now }, now)).catch(() => {});
}

function edgeHeard(raw) {
  let message;
  try { message = JSON.parse(typeof raw === 'string' ? raw : ''); } catch { return; }
  if (!message || typeof message !== 'object') return;
  if (Number.isSafeInteger(message.seq) && message.seq > edge.seq) edge.seq = message.seq;
  if (message.t === 'hello') {
    edge.ready = true;
    // The server restarted, or this phone is ahead: read the authoritative data
    // again when it is next needed.
    if (message.reset) savedMatches = { at: 0, data: null };
    if (message.state?.body) {
      edge.peerAt = Date.now();
      edgeApply(message.state.body);
    }
    return;
  }
  if (message.from === 'computer') edge.peerAt = Date.now();
  if (message.t === 'state') {
    edgeApply(message);
  } else if (message.t === 'reply') {
    const waiter = edge.waits.get(message.id);
    if (!waiter) return;
    edge.waits.delete(message.id);
    clearTimeout(waiter.timer);
    if (typeof message.error === 'string') waiter.reject(answered(message.error));
    else {
      const { v, t, id, seq, from, duplicate, ...answer } = message;
      waiter.resolve(answer);
    }
  } else if (message.t === 'control') {
    sinks.control?.(message);
  }
}

function edgeLater() {
  if (edge.timer) return;
  const wait = EDGE_RETRY_MS[Math.min(edge.retry, EDGE_RETRY_MS.length - 1)];
  edge.retry++;
  edge.timer = setTimeout(() => { edge.timer = null; edgeLive(); }, wait);
}

/** Opens the edge socket, or leaves the one that is already open alone. */
export async function edgeLive(fresh = false) {
  if (!localStorage.getItem(STORE.secret)) return;
  const socket = edge.socket;
  if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) return;
  const tokens = await edgeTokens(fresh);
  if (!tokens) return;
  let url;
  try {
    url = new URL('/v1/live', tokens.url);
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    url.searchParams.set('token', tokens.live);
    if (edge.seq) url.searchParams.set('since', String(edge.seq));
  } catch { return; }
  let opening;
  try { opening = new WebSocket(url.href); } catch { return; }
  edge.socket = opening;
  edge.opened = false;
  opening.addEventListener('open', () => {
    if (edge.socket !== opening) return;
    edge.opened = true;
    edge.ready = true;
    edge.retry = 0;
  });
  opening.addEventListener('message', event => { if (edge.socket === opening) edgeHeard(event.data); });
  opening.addEventListener('close', () => {
    if (edge.socket !== opening) return;
    edge.socket = null;
    edge.ready = false;
    // A refused handshake is usually an old token: ask for a fresh one.
    if (!edge.opened) edge.at = 0;
    edge.opened = false;
    edgeLater();
  });
  opening.addEventListener('error', () => { /* a close follows; never a page error */ });
}

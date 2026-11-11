// Playing on this phone, and the player every screen shows.
//
// Every song plays in this page; the computer only says where its audio is. A
// Spotify song plays as the same song from YouTube or SoundCloud, which the
// computer finds, so a mixed playlist runs straight through. Nothing here
// leaves the page to play sound.

import { STORE } from './auth.js';
import { toast, uuid } from './dom.js';
import { answered, SignedOut } from './errors.js';
import { api } from './service.js';
import { hasSpotify, spotifySong, web } from './spotify.js';
import { edge, edgeLive, edgePeer, edgeResolve, edgeSend, edgeSpotifyStream, edgeSoundcloudSource, edgeUpload, edgeWarm, setEdgeSinks } from './edge.js';
import { act, ask } from './remote.js';
import { lockScreenHandlers, lockScreenState, mediaSession } from './audio.js';
import { shuffled } from './order.js';
import { events, here, hooks, noteFirstLook, player, readyReplies, replyWaiters, told } from './state.js';

const TARGET_KEY = 'encore.target';
const HERE_KEY = 'encore.here';
const HERE_KEEP = 500;
export const playsHere = () => localStorage.getItem(TARGET_KEY) === 'phone';

/** The same song comes back after 1s, 2s, 4s, then every 8s until the phone is back. */
const RETRY_MS = [1000, 2000, 4000];
const OFFLINE_RETRY_MS = 8000;
/** Ten seconds with nothing arriving means start the same song again. */
const STALL_MS = 10000;
let retryTimer = null;
let stallTimer = null;

function hereAudio() {
  if (here.audio) return here.audio;
  const audio = new Audio();
  audio.preload = 'auto';
  audio.addEventListener('ended', () => {
    if (sleep.song) {
      sleepAfter(null);
      here.place = 0;
      here.pausedByUser = true;
      saveHere();
      refreshPlayer();
      return;
    }
    const next = nextIndex(1, true);
    if (next < 0) { here.place = 0; saveHere(); refreshPlayer(); return; }
    playIndex(next);
  });
  // A failure the page can mend itself is never a skipped song.
  audio.addEventListener('error', () => {
    if (!here.active || here.trouble || !audio.src) return;
    if (!here.pending && here.playingUri === here.list[here.index]?.uri && audio.currentTime > 0) here.place = audio.currentTime * 1000;
    retryHere();
  });
  audio.addEventListener('pause', () => {
    if (!here.pending && here.playingUri === here.list[here.index]?.uri && audio.currentTime > 0) here.place = audio.currentTime * 1000;
    saveHere();
  });
  audio.addEventListener('playing', () => {
    here.tries = 0;
    here.pending = false;
    here.pausedByUser = false;
    // Something plays, so every song is worth trying again.
    here.dead.clear();
    clearTimeout(stallTimer);
    lockScreenHandlers();
    lockScreenState();
    saveHere();
  });
  for (const event of ['play', 'pause', 'loadedmetadata']) audio.addEventListener(event, refreshPlayer);
  for (const event of ['play', 'pause', 'seeked']) audio.addEventListener(event, () => tellComputer(true));
  for (const event of ['playing', 'timeupdate']) audio.addEventListener(event, () => clearTimeout(stallTimer));
  for (const event of ['stalled', 'waiting']) audio.addEventListener(event, stallSoon);
  // A locked phone slows a page's timers down; the song's own clock keeps time.
  audio.addEventListener('timeupdate', () => { if (sleep.until && Date.now() >= sleep.until) sleepNow(); });
  here.audio = audio;
  lockScreenHandlers();
  return audio;
}

// ---------------------------------------------------------------- the sleep timer
// Only this phone's own player has one. A page that plays keeps running while
// the phone is locked, which a page that only watches the computer does not.
const sleep = { until: 0, song: false, timer: null };

/** What the sleep timer is set to: `{ minutes }` still to go, `{ song: true }`, or null. */
export function sleepSet() {
  if (sleep.song) return { song: true };
  return sleep.until > Date.now() ? { minutes: Math.max(1, Math.ceil((sleep.until - Date.now()) / 60000)) } : null;
}

/** Sets it: a number of minutes, 'song' for the end of this song, or null for off. */
export function sleepAfter(when) {
  clearTimeout(sleep.timer);
  sleep.timer = null;
  sleep.until = 0;
  sleep.song = when === 'song';
  if (typeof when === 'number' && when > 0) {
    sleep.until = Date.now() + when * 60000;
    sleep.timer = setTimeout(sleepNow, when * 60000);
  }
}

/** Time is up: the music pauses where it is, as if the listener had pressed it. */
function sleepNow() {
  sleepAfter(null);
  const audio = here.audio;
  if (!here.active || !audio || audio.paused) return;
  here.pausedByUser = true;
  clearTimeout(retryTimer);
  retryTimer = null;
  audio.pause();
}

/** Stuck with nothing arriving: give it ten seconds, then start the same song again. */
function stallSoon() {
  clearTimeout(stallTimer);
  stallTimer = setTimeout(() => {
    const audio = here.audio;
    if (!here.active || here.trouble || !audio || audio.paused) return;
    if (here.playingUri === here.list[here.index]?.uri) here.place = audio.currentTime * 1000;
    retryHere();
  }, STALL_MS);
}

/** The next song: this one again while repeating it, else along the list. */
function nextIndex(step = 1, ended = false) {
  if (!here.list.length) return -1;
  if (ended && here.repeat === 'one') return here.index;
  const at = here.index + step;
  if (at >= here.list.length) return here.repeat === 'all' ? 0 : -1;
  if (at < 0) return 0;
  return at;
}

/** Where the song is now: the audio's own place, or the one it is going to. */
function herePosition() {
  if (here.pending) return here.place;
  const audio = here.audio;
  const mine = here.playingUri === here.list[here.index]?.uri;
  return audio && mine && audio.currentTime > 0 ? audio.currentTime * 1000 : here.place;
}

/** Whether this very song is liked: what the computer said, or what was just pressed here. */
function hereLiked(song) {
  if (here.likedUri === song?.uri) return here.liked;
  return player.state?.song?.uri === song?.uri && !!player.state.liked;
}

/**
 * Plays `list` here from `index`. From a tap it starts the element before anything is
 * awaited, because iOS only lets sound begin inside that tap.
 */
function playHere(list, index, tap = false) {
  here.list = list;
  here.index = -1;
  here.active = true;
  here.trouble = null;
  here.tries = 0;
  here.place = 0;
  // A tap is the listener asking again, whatever failed last time.
  here.dead.clear();
  if (tap) {
    const audio = hereAudio();
    const known = streams.get(list[index]?.uri)?.url;
    if (known) { audio.src = known; here.playingUri = list[index]?.uri; }
    // Straight away, in the tap itself, even when there's nothing to play yet.
    Promise.resolve(audio.play()).catch(() => {});
  }
  saveHere();
  return playIndex(index);
}

/** Stops playing here, and forgets what was left for a reload. */
export function stopHere() {
  clearTimeout(retryTimer);
  clearTimeout(stallTimer);
  retryTimer = null;
  sleepAfter(null);
  here.seq++;
  here.audio?.pause();
  here.active = false;
  here.trouble = null;
  here.list = [];
  here.index = -1;
  here.place = 0;
  here.playingUri = null;
  here.pending = false;
  try { localStorage.removeItem(HERE_KEY); } catch { /* nothing to forget */ }
  if ('mediaSession' in navigator) {
    navigator.mediaSession.playbackState = 'none';
    navigator.mediaSession.metadata = null;
  }
  refreshPlayer();
  tellComputer();
}

/** Starts a song here. `at` is where to resume it, in ms. */
async function playIndex(index, { at = 0 } = {}) {
  if (index < 0 || index >= here.list.length) {
    here.audio?.pause();
    refreshPlayer();
    saveHere();
    return;
  }
  here.index = index;
  here.trouble = null;
  here.place = at;
  here.pending = true;
  here.pausedByUser = false;
  const song = here.list[index];
  mediaSession(song);
  refreshPlayer();
  markPlaying();
  tellComputer(true);
  ahead();
  saveHere();
  await loadHere();
}

/** The next two songs' addresses, so this one ending starts them with no wait. */
function ahead() {
  const next = here.list.slice(here.index + 1, here.index + 3);
  for (const song of next) streamOf(song).catch(() => {});
  // The edge fetches them ahead too, so its answer is ready when they play.
  if (next.length) edgeWarm(next).catch(() => {});
}

/** Asks where the song is, then plays it where it was. A retry asks again: it may have expired. */
async function loadHere(fresh = false) {
  const song = here.list[here.index];
  if (!song || !here.active) return;
  const mine = ++here.seq;
  clearTimeout(retryTimer);
  retryTimer = null;
  try {
    const found = await streamOf(song, fresh);
    if (here.seq !== mine || !here.active) return;
    if (!found?.url) throw answered('No audio for this song.');
    const audio = hereAudio();
    if (found.hls && !audio.canPlayType('application/vnd.apple.mpegurl')) throw answered("this browser can't play it");
    // Already started by the tap, from this very address: leave it running.
    if (!audio.paused && !audio.ended && !audio.error && audio.src === found.url && audio.readyState > 0) return;
    const at = here.place;
    if (at > 0) audio.addEventListener('loadedmetadata', () => { audio.currentTime = at / 1000; }, { once: true });
    audio.src = found.url;
    here.playingUri = song.uri;
    here.pending = true;
    // Paused while the address was on its way: it stays paused, and ready.
    if (here.pausedByUser) { here.pending = false; refreshPlayer(); saveHere(); return; }
    await audio.play();
  } catch (error) {
    if (here.seq !== mine || !here.active) return;
    if (error?.answered) return deadHere(song, error.message);
    retryHere();
  }
}

/** Tries the same song, at the same place, again. Never a different song. */
function retryHere() {
  if (!here.active || retryTimer) return;
  const wait = here.tries < RETRY_MS.length ? RETRY_MS[here.tries] : OFFLINE_RETRY_MS;
  here.tries++;
  retryTimer = setTimeout(() => {
    retryTimer = null;
    // A pause the listener asked for stays paused.
    if (here.active && !here.pausedByUser) loadHere(true);
  }, wait);
  refreshPlayer();
}

// Back on a network: the retry it was waiting for happens straight away.
addEventListener('online', () => {
  // The edge reconnects at once too, with a fresh token.
  clearTimeout(edge.timer);
  edge.timer = null;
  edge.retry = 0;
  edgeLive(true);
  if (!retryTimer || here.pausedByUser) return;
  clearTimeout(retryTimer);
  retryTimer = null;
  loadHere(true);
});

/** The computer says there's no audio for this song: show that, and stop here. */
function stuckHere(song, reason) {
  clearTimeout(retryTimer);
  clearTimeout(stallTimer);
  retryTimer = null;
  here.audio?.pause();
  here.trouble = { uri: song.uri, name: song.name, reason: reason || "This song can't play on this phone." };
  refreshPlayer();
  tellComputer(true);
  saveHere();
}

/**
 * The service answered that this song has no audio here. That answer will not
 * change by asking again, so the song after it plays: nobody on a bike wants
 * to be asked, and the music stopping is the one thing worse than a missing
 * song. With nothing left to play, the reason stays on screen instead.
 */
function deadHere(song, reason) {
  here.dead.add(song.uri);
  const next = nextIndex(1);
  const after = next >= 0 ? here.list[next] : null;
  if (!after || here.dead.has(after.uri)) return stuckHere(song, reason);
  toast(`Skipped ${song.name}: no audio for it`);
  return playIndex(next);
}

/** How the phone's own playback shows, in the shape the player already uses. */
function hereState() {
  const song = here.list[here.index];
  if (!song) return null;
  const audio = here.audio;
  return {
    song: { ...song, duration_ms: song.duration_ms || (here.playingUri === song.uri && audio?.duration ? audio.duration * 1000 : 0) },
    position_ms: herePosition(), playing: !!audio && here.playingUri === song.uri && !audio.paused,
    liked: hereLiked(song), shuffle: here.shuffle, repeat: here.repeat,
    volume: Math.round((audio?.volume ?? 1) * 100), device: 'This phone',
    next: here.list.slice(here.index + 1, here.index + 6), here: true, trouble: here.trouble,
    // Everything still to come, for the queue; and whether the song is on its way.
    upcoming: here.list.slice(here.index + 1), loading: here.pending && !here.pausedByUser && !here.trouble,
    retrying: here.tries > 0 && !here.trouble,
  };
}

/** Remembers the queue and the place, so a reload carries on where it was, paused. */
function saveHere() {
  if (!here.active || !here.list.length) return;
  here.place = Math.round(herePosition());
  // At most 500 songs travel, with the one playing in the middle of them.
  const half = Math.floor(HERE_KEEP / 2);
  const from = Math.max(0, Math.min(here.index - half, here.list.length - HERE_KEEP));
  try {
    localStorage.setItem(HERE_KEY, JSON.stringify({
      list: here.list.slice(from, from + HERE_KEEP), index: here.index - from, position: here.place,
      target: localStorage.getItem(TARGET_KEY), shuffle: here.shuffle, repeat: here.repeat,
    }));
  } catch { /* no room, or no storage: playback carries on all the same */ }
}

/** Puts the queue and place back after a reload: paused, and nothing starts by itself. */
export function restoreHere() {
  if (!playsHere()) return;
  let saved;
  try { saved = JSON.parse(localStorage.getItem(HERE_KEY) ?? 'null'); } catch { return; }
  if (!Array.isArray(saved?.list) || !saved.list.length) return;
  here.list = saved.list.slice(0, HERE_KEEP);
  here.index = Math.min(Math.max(0, Number(saved.index) || 0), here.list.length - 1);
  here.place = Math.max(0, Number(saved.position) || 0);
  here.shuffle = !!saved.shuffle;
  here.repeat = saved.repeat === 'one' || saved.repeat === 'all' ? saved.repeat : 'off';
  here.active = true;
  mediaSession(here.list[here.index]);
  refreshPlayer();
}
setInterval(() => { if (here.active) saveHere(); }, 3000);
addEventListener('pagehide', saveHere);
document.addEventListener('visibilitychange', () => { if (document.hidden) saveHere(); });

/** Where a song's audio is, asked once per song for a while. */
const STREAM_KEEP_MS = 10 * 60 * 1000;
const streams = new Map();
function streamOf(song, fresh = false) {
  const held = streams.get(song.uri);
  if (!fresh && held && Date.now() - held.at < STREAM_KEEP_MS) return held.found;
  const found = findStream(song);
  const entry = { at: Date.now(), found, url: null };
  streams.set(song.uri, entry);
  found.then(value => { if (streams.get(song.uri) === entry) entry.url = value?.url ?? null; },
    () => { if (streams.get(song.uri) === entry) streams.delete(song.uri); });
  return found;
}

/**
 * The edge first when it can speak for a song, then the computer exactly as
 * before. A few guidelines live here:
 * - SoundCloud: only a public source the edge accepts is sent; a refusal just
 *   asks the computer.
 * - Spotify: a saved match outranks a fresh search, and a failed resolve asks
 *   the computer.
 * - YouTube: a connected computer answers first; without one, the edge is the
 *   only chance, and it failing is the normal "can't play here" state.
 * - Group links and anything else: the computer, as today.
 */
async function findStream(song) {
  const uri = String(song.uri ?? '');
  const question = uri.startsWith('spotify:')
    ? { uri, name: song.name, artists: song.artists, duration_ms: song.duration_ms }
    : { uri };
  // A mixed playlist's local file lives on the edge, so the edge answers first.
  if (typeof song.upload === 'string' && song.upload) {
    const found = await edgeUpload(song.upload).catch(() => null);
    if (found) return found;
  }
  if (uri.startsWith('youtube:')) {
    if (player.online) return await ask('stream', question);
    const found = await edgeResolve(uri).catch(() => null);
    if (found) return found;
    throw answered("This song can't play on this phone right now.");
  }
  if (uri.startsWith('soundcloud:')) {
    const source = edgeSoundcloudSource(song);
    const found = source ? await edgeResolve(source).catch(() => null) : null;
    if (found) return found;
  } else if (uri.startsWith('spotify:')) {
    const found = await edgeSpotifyStream(song).catch(() => null);
    if (found) return found;
  } else {
    // Group links and anything else keep asking the computer, exactly as today.
    return await ask('stream', question);
  }
  return await askComputer(question);
}

/**
 * The computer's answer, or the page's plain stuck state when it is not there
 * either. A timeout while it is online still retries as before.
 */
async function askComputer(question) {
  try { return await ask('stream', question); }
  catch (error) {
    if (!player.online && !(error instanceof SignedOut) && !error?.answered) {
      throw answered("This song can't play on this phone right now.");
    }
    throw error;
  }
}

// Addresses don't last: ask for the next ones again before they could go stale.
setInterval(() => {
  if (!here.active) return;
  const soon = STREAM_KEEP_MS - 60 * 1000;
  for (const song of here.list.slice(here.index + 1, here.index + 3)) {
    const held = streams.get(song.uri);
    if (held && Date.now() - held.at > soon) streamOf(song, true).catch(() => {});
  }
}, 60 * 1000);

// ---------------------------------------------------------------- telling the computer
// What plays on this phone shows on the computer too, as Spotify shows another
// device, so the listener sees it there and can stop it or take it back.
const TELL_EVERY_MS = 400;
/** Tells the computer what this phone plays now; quickly, but not more than it needs. */
function tellComputer(force = false) {
  if (!localStorage.getItem(STORE.secret)) return;
  const state = here.active ? hereState() : null;
  if (!state) {
    if (told.key) {
      told.key = '';
      if (!edgeSend({ t: 'phone', id: uuid(), phone: null })) api('/live/remote/phone', 'DELETE').catch(() => {});
    }
    return;
  }
  const key = `${state.song.uri}|${state.playing}`;
  // A song going on as expected needs no word: the computer counts the time itself.
  // A new song, a pause, a seek or a long while does.
  const due = force || key !== told.key || Date.now() - told.at > 30000;
  if (!due) return;
  if (Date.now() - told.at < TELL_EVERY_MS) {
    clearTimeout(told.timer);
    told.timer = setTimeout(() => tellComputer(true), TELL_EVERY_MS);
    return;
  }
  told.at = Date.now();
  told.key = key;
  const { song } = state;
  const phone = {
    song: { uri: song.uri, name: song.name, artists: song.artists, image: song.image ?? null, duration_ms: Math.round(song.duration_ms || 0) },
    playing: state.playing, position_ms: Math.round(state.position_ms), device: state.device,
  };
  if (edgeSend({ t: 'phone', id: uuid(), phone })) return;
  api('/live/remote/phone', 'PUT', phone).catch(() => {});
}
setInterval(() => { if (here.active) tellComputer(); }, 2000);

const HERE_ACTS = {
  toggle: () => {
    const audio = hereAudio();
    // A press after the computer said no asks it once more.
    if (here.trouble) { here.trouble = null; here.pausedByUser = false; return loadHere(true); }
    if (!audio.src) { here.pausedByUser = false; return playIndex(here.index, { at: here.place }); }
    if (audio.paused) { here.pausedByUser = false; if (audio.ended) audio.currentTime = 0; return audio.play(); }
    here.pausedByUser = true;
    clearTimeout(retryTimer);
    retryTimer = null;
    return audio.pause();
  },
  next: () => {
    const next = nextIndex(1);
    if (next < 0) { here.audio?.pause(); saveHere(); refreshPlayer(); return; }
    return playIndex(next);
  },
  previous: () => {
    if (herePosition() > 3000) return HERE_ACTS.seek({ ms: 0 });
    return playIndex(Math.max(0, here.index - 1));
  },
  seek: ({ ms }) => {
    here.place = Math.max(0, ms);
    here.pending = false;
    const audio = hereAudio();
    if (audio.src) audio.currentTime = here.place / 1000;
    lockScreenState();
  },
  volume: ({ percent }) => { hereAudio().volume = Math.min(1, Math.max(0, percent / 100)); },
  shuffle: () => {
    here.shuffle = !here.shuffle;
    // The song playing stays and the rest of the list goes round.
    const song = here.list[here.index];
    if (here.shuffle && song) {
      here.list = [song, ...shuffled(here.list.filter((_, at) => at !== here.index))];
      here.index = 0;
    }
    saveHere();
    refreshPlayer();
  },
  repeat: () => {
    here.repeat = here.repeat === 'off' ? 'all' : here.repeat === 'all' ? 'one' : 'off';
    saveHere();
    refreshPlayer();
  },
  // The queue is the listener's own work: these only ever move, add or take out
  // one song, and never touch the one playing. `at` counts from the next song.
  queue: ({ song, at = 0 }) => {
    if (!song) return;
    const index = Math.min(here.list.length, here.index + 1 + Math.max(0, at));
    here.list = [...here.list.slice(0, index), song, ...here.list.slice(index)];
    queueChanged();
  },
  last: ({ song }) => {
    if (!song) return;
    here.list = [...here.list, song];
    queueChanged();
  },
  jump: ({ at }) => {
    const index = here.index + 1 + at;
    if (index <= here.index || index >= here.list.length) return;
    return playIndex(index);
  },
  move: ({ from, to }) => {
    const base = here.index + 1;
    const coming = here.list.slice(base);
    if (from === to || from < 0 || to < 0 || from >= coming.length || to >= coming.length) return;
    const [song] = coming.splice(from, 1);
    coming.splice(to, 0, song);
    here.list = [...here.list.slice(0, base), ...coming];
    queueChanged();
  },
  remove: ({ at }) => {
    const index = here.index + 1 + at;
    if (at < 0 || index >= here.list.length) return null;
    const song = here.list[index];
    here.list = [...here.list.slice(0, index), ...here.list.slice(index + 1)];
    queueChanged();
    return song;
  },
  clear: () => {
    const taken = here.list.slice(here.index + 1);
    here.list = here.list.slice(0, here.index + 1);
    queueChanged();
    return taken;
  },
  restore: ({ songs }) => {
    here.list = [...here.list.slice(0, here.index + 1), ...songs];
    queueChanged();
  },
};

/** After the order changed: keep it, fetch what is now next, and show it. */
function queueChanged() {
  saveHere();
  ahead();
  refreshPlayer();
}

/** Hands the song back to the computer and stops here. */
export function playOnComputer(song) {
  localStorage.setItem(TARGET_KEY, 'computer');
  stopHere();
  act('play', { uris: [song.uri], index: 0 }).catch(() => toast("Encore isn't open on your computer."));
}

/** Where the sound comes from: 'phone' or 'computer'. Moving keeps the queue and the place. */
export function chooseTarget(target) {
  if (target === 'computer') {
    if (!playsHere()) return;
    const state = here.active ? hereState() : null;
    localStorage.setItem(TARGET_KEY, 'computer');
    stopHere();
    // What was playing here carries on there, where it can.
    if (state?.song && player.online) {
      const uris = [state.song, ...state.upcoming].slice(0, 500).map(song => song.uri);
      act('play', { uris, index: 0 }).then(() => act('seek', { ms: Math.round(state.position_ms) })).catch(() => toast("Couldn't reach Encore."));
    }
    refreshPlayer();
    return;
  }
  if (playsHere() && here.active) return;
  localStorage.setItem(TARGET_KEY, 'phone');
  // Whatever the computer has queued carries on here, from the same place.
  const state = player.state;
  if (state?.song && player.online) {
    const list = state.queue?.length ? [...state.queue] : [state.song, ...(state.next ?? [])];
    const start = Math.max(0, list.findIndex(item => item.uri === state.song.uri));
    if (state.playing) api('/live/remote/send', 'POST', { id: uuid(), kind: 'toggle' }).catch(() => {});
    playHere(list, start, true).then(() => HERE_ACTS.seek({ ms: position(state) }));
  }
  refreshPlayer();
}

// ---------------------------------------------------------------- the player

/** What the listener just did shows at once and stays until the computer agrees or time's up. */
const HOLD_MS = 4000;
export function hold(field, value) {
  player.held.set(field, { value, until: Date.now() + HOLD_MS });
  refreshPlayer();
}
/** What the page shows: the phone's own player while it plays, else the computer's. */
export function shown() {
  const state = here.active ? hereState() : player.state ? { ...player.state } : null;
  if (!state) return null;
  for (const [field, { value, until }] of player.held) {
    if (Date.now() < until && state[field] !== value) state[field] = value;
    else player.held.delete(field);
  }
  return state;
}
/** Where the song is, in ms, whichever device plays it. */
export const placeOf = state => (state?.here ? state.position_ms : position(state));
function position(state) {
  if (!state) return 0;
  const moved = state.playing ? Date.now() - player.at : 0;
  return Math.min(state.position_ms + moved, state.song?.duration_ms ?? Infinity);
}

/** Takes in the computer's state, from a look or from the event stream. */
async function applyState(state, now) {
  // While a computer is on the edge, its newer word outranks an older relay look.
  if (state && Number.isFinite(state.at) && edgePeer() && state.at + 1000 < edge.stateAt) {
    player.online = true;
    noteFirstLook();
    refreshPlayer();
    return;
  }
  const fresh = state && now - state.at < 30000;
  const wasOnline = player.online;
  player.online = !!fresh;
  noteFirstLook();
  if (fresh) {
    const sought = player.sought;
    player.state = state;
    player.direct = false;
    player.at = Date.now() - Math.max(0, now - state.at);
    // An answer from before a seek still has the old place: keep the new one a while.
    if (sought && Date.now() < sought.until && sought.uri === state.song?.uri) {
      const expected = sought.ms + (state.playing ? Date.now() - sought.at : 0);
      if (Math.abs(position(state) - expected) > 2500) {
        player.state = { ...state, position_ms: expected };
        player.at = Date.now();
      } else player.sought = null;
    }
  } else if (hasSpotify()) {
    await pollSpotify();
  } else {
    player.direct = false;
  }
  if (wasOnline !== player.online) {
    hooks.renderBanner?.();
    // Only Spotify's own answers differ from the computer's, so nothing else reloads.
    if (booted && hasSpotify()) hooks.reloadViews?.();
  }
  booted = true;
  refreshPlayer();
}

export async function pollState() {
  if (!document.hidden && localStorage.getItem(STORE.secret)) {
    try {
      const { state, now } = await api('/live/remote/state');
      await applyState(state, now);
    } catch (error) {
      if (error instanceof SignedOut) { hooks.signedOut?.(); return; }
    }
  }
  // With the event stream open, a look is only a safety net.
  const every = events.open ? 6000 : player.online || !hasSpotify() ? 1200 : 3000;
  setTimeout(pollState, every);
}
let booted = false;

// ---------------------------------------------------------------- hearing the computer at once
// While this page is in front, the computer's every change, every ready answer and
// every word for the phone's own player arrive over one event stream, instead of at
// the next look. The service closes it every few minutes; it's opened again at once.
export async function listenComputer() {
  if (events.stop || document.hidden || !localStorage.getItem(STORE.secret)) return;
  const stop = new AbortController();
  events.stop = stop;
  let failed = false;
  try {
    const secret = localStorage.getItem(STORE.secret);
    const group = localStorage.getItem(STORE.group);
    const answer = await fetch('/api/live/remote/events?for=phone', {
      headers: { authorization: `Bearer ${secret}`, ...(group ? { 'x-siphon-group': group } : {}), accept: 'text/event-stream' },
      signal: stop.signal, cache: 'no-store',
    });
    if (!answer.ok || !answer.body) throw new Error('no stream');
    events.open = true;
    const reader = answer.body.pipeThrough(new TextDecoderStream()).getReader();
    let pending = '';
    while (true) {
      const part = await reader.read();
      if (part.done) break;
      pending += part.value;
      let end;
      while ((end = pending.indexOf('\n\n')) >= 0) {
        const event = pending.slice(0, end);
        pending = pending.slice(end + 2);
        const data = event.split('\n').find(line => line.startsWith('data: '));
        if (data) heard(JSON.parse(data.slice(6)));
      }
    }
  } catch { failed = !stop.signal.aborted; }
  events.open = false;
  events.stop = null;
  // Closed by the service on time: straight back. Refused or cut off: give it a moment.
  if (!stop.signal.aborted) setTimeout(listenComputer, failed ? 5000 : 0);
}

function heard(message) {
  if (message.t === 'state') applyState(message.state, message.now ?? Date.now()).catch(() => {});
  else if (message.t === 'reply') {
    const wake = replyWaiters.get(message.id);
    if (wake) wake();
    else if (readyReplies.size < 50) {
      // Ready before it was waited for: the wait skips straight to it.
      readyReplies.add(message.id);
      setTimeout(() => readyReplies.delete(message.id), 60000);
    }
  }
  else if (message.t === 'control') controlHere(message);
}

/** The computer working this phone's own player. */
function controlHere({ kind, ms }) {
  if (!here.active) return;
  const playing = hereState()?.playing;
  if (kind === 'pause' && !playing) return;
  if (kind === 'play' && playing) return;
  const act = kind === 'pause' || kind === 'play' ? 'toggle' : kind;
  Promise.resolve(HERE_ACTS[act]?.({ ms })).catch(() => {});
}

document.addEventListener('visibilitychange', () => {
  // Coming back to the page is a fresh chance for the edge as well.
  edgeLive();
  if (document.hidden) events.stop?.abort();
  else { listenComputer(); if (here.active) tellComputer(true); }
});

/** With the computer off, what's playing is whatever Spotify says, on any device. */
async function pollSpotify() {
  try {
    const now = await web('/me/player');
    player.direct = true;
    if (!now?.item) { player.state = null; return; }
    player.state = {
      song: spotifySong(now.item), liked: player.state?.song?.uri === now.item.uri ? player.state.liked : false,
      position_ms: now.progress_ms ?? 0, playing: now.is_playing, shuffle: now.shuffle_state, repeat: now.repeat_state === 'off' ? 'off' : 'all',
      volume: now.device?.volume_percent ?? 50, device: now.device?.name ?? 'Spotify', next: [],
    };
    player.at = Date.now();
    refreshPlayer();
  } catch { /* the next look tries again */ }
}

/** Draws the player again, wherever it shows: the mini player, the sheet, the lock screen. */
export function refreshPlayer() {
  const state = shown();
  player.mini?.update(state, !!state?.song && (player.online || player.direct || here.active));
  lockScreenState();
  player.sheet?.update(state);
}
/** Marks the song playing, wherever it's listed on the page. */
export function markPlaying() {
  const state = shown();
  const uri = state?.song?.uri;
  document.querySelectorAll('.row[data-uri]').forEach(row => {
    const mine = row.dataset.uri === uri;
    row.classList.toggle('playing', mine);
    row.classList.toggle('paused', mine && !state.playing);
  });
}
/** A small knock under the thumb, where the phone can. */
const knock = () => navigator.vibrate?.(8);
/** Moves to `ms` now, and holds it there until the computer agrees. */
export function seekTo(ms) {
  if (here.active) { act('seek', { ms }); return; }
  if (!player.state) return;
  player.state.position_ms = ms;
  player.at = Date.now();
  player.sought = { ms, at: Date.now(), until: Date.now() + HOLD_MS, uri: player.state.song?.uri };
  act('seek', { ms });
}
export function togglePlay() {
  knock();
  const state = shown();
  hold('playing', !state?.playing);
  act('toggle').catch(() => toast("Couldn't reach Encore."));
}
setInterval(() => { if (!document.hidden) refreshPlayer(); }, 500);

/** Next or previous, from a button or a swipe. */
export function skip(step) {
  knock();
  return Promise.resolve(act(step > 0 ? 'next' : 'previous')).catch(() => toast("Couldn't reach Encore."));
}
/** Likes or unlikes what is playing, shown at once. */
export function likePlaying() {
  const state = shown();
  if (!state?.song) return;
  hold('liked', !state.liked);
  if (here.active) { here.likedUri = state.song.uri; here.liked = !state.liked; }
  Promise.resolve(act('like', { uri: state.song.uri })).catch(() => toast("Couldn't reach Encore."));
}
export function flipShuffle() {
  hold('shuffle', !shown()?.shuffle);
  Promise.resolve(act('shuffle')).catch(() => toast("Couldn't reach Encore."));
}
export function turnRepeat() {
  Promise.resolve(act('repeat')).catch(() => toast("Couldn't reach Encore."));
}
/** The computer's volume. A phone's own is its side buttons, so there is none to set here. */
export function setVolume(percent) {
  hold('volume', percent);
  Promise.resolve(act('volume', { percent })).catch(() => {});
}
/** A song that can't play here: move on to the next one. */
export function skipTrouble() {
  here.trouble = null;
  return HERE_ACTS.next();
}
/** Puts a song right after the one playing, on whichever device plays. */
export function playNext(song) {
  if (here.active) { HERE_ACTS.queue({ song }); return Promise.resolve(); }
  return Promise.resolve(act('queue', { uri: song.uri }));
}
/** Whether songs can be added to the end, moved and taken out: only this phone's own queue can. */
export const ownsQueue = () => here.active;

// ---------------------------------------------------------------- rows and actions
export function playFrom(list, index) {
  if (playsHere()) { playHere(list, index, true); return; }
  if (!player.online && !hasSpotify()) { toast('Open Encore on your computer first.'); return; }
  // The computer plays 500 at most, so a very long list starts around the song.
  const start = Math.max(0, index - 100);
  const uris = list.slice(start, start + 500).map(song => song.uri);
  // Shown straight away; the computer's next word confirms it.
  player.state = { ...(player.state ?? { repeat: 'off', volume: 50, next: [] }), song: list[index], position_ms: 0, playing: true };
  player.at = Date.now();
  refreshPlayer();
  act('play', { uris, index: index - start }).catch(() => toast("Couldn't reach Encore."));
}

// The few calls another module needs, and the player's own part in the edge.
hooks.hereActs = HERE_ACTS;
hooks.playsHere = playsHere;
hooks.markPlaying = markPlaying;
hooks.playFrom = playFrom;
hooks.shown = shown;
hooks.pollSpotify = pollSpotify;
setEdgeSinks(applyState, controlHere);

// iOS only lets sound begin inside a tap, so the element is here from the start.
hereAudio();

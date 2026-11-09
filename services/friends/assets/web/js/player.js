// Playing on this phone, and the player every screen shows.
//
// Every song plays in this page; the computer only says where its audio is. A
// Spotify song plays as the same song from YouTube or SoundCloud, which the
// computer finds, so a mixed playlist runs straight through. Nothing here
// leaves the page to play sound.

import { STORE } from './auth.js';
import { art, bones, clock, h, icon, safeImage, sourceOf, toast, uuid } from './dom.js';
import { answered, SignedOut } from './errors.js';
import { api } from './service.js';
import { hasSpotify, spotifySong, web } from './spotify.js';
import { edge, edgeLive, edgePeer, edgeResolve, edgeSend, edgeSpotifyStream, edgeSoundcloudSource, edgeWarm, setEdgeSinks } from './edge.js';
import { act, ask } from './remote.js';
import { lockScreenHandlers, lockScreenState, mediaSession } from './audio.js';
import { row, shareSong, shuffled } from './rows.js';
import { go, over, overClosed } from './navigation.js';
import { events, here, hooks, player, readyReplies, replyWaiters, told } from './state.js';

const TARGET_KEY = 'encore.target';
const HERE_KEY = 'encore.here';
const HERE_KEEP = 500;
const playsHere = () => localStorage.getItem(TARGET_KEY) === 'phone';

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
    clearTimeout(stallTimer);
    lockScreenHandlers();
    lockScreenState();
    saveHere();
  });
  for (const event of ['play', 'pause', 'loadedmetadata']) audio.addEventListener(event, refreshPlayer);
  for (const event of ['play', 'pause', 'seeked']) audio.addEventListener(event, () => tellComputer(true));
  for (const event of ['playing', 'timeupdate']) audio.addEventListener(event, () => clearTimeout(stallTimer));
  for (const event of ['stalled', 'waiting']) audio.addEventListener(event, stallSoon);
  here.audio = audio;
  lockScreenHandlers();
  return audio;
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
    if (error?.answered) return stuckHere(song, error.message);
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
};

/** Hands the song back to the computer and stops here. */
function playOnComputer(song) {
  localStorage.setItem(TARGET_KEY, 'computer');
  stopHere();
  act('play', { uris: [song.uri], index: 0 }).catch(() => toast("Encore isn't open on your computer."));
}

/** Where the sound comes from: the computer or this phone. */
function targetPicker() {
  const scrim = h('div', { class: 'scrim', onclick: () => close() });
  const close = () => { scrim.remove(); panel.remove(); overClosed(); };
  const choose = (label, chosen, run) => h('button', { text: `${chosen ? '♪ ' : ''}${label}`, onclick: () => { close(); run(); } });
  const onPhone = playsHere();
  const panel = h('div', { class: 'actions', role: 'menu' },
    h('div', { class: 'about' }, h('b', { text: 'Play on' }), h('span', { text: 'Everything plays on the phone itself, Spotify songs included.' })),
    choose('Your computer', !onPhone, () => {
      localStorage.setItem(TARGET_KEY, 'computer');
      stopHere();
    }),
    choose('This phone', onPhone, () => {
      localStorage.setItem(TARGET_KEY, 'phone');
      // Whatever the computer has queued carries on here, from the same place.
      const state = player.state;
      if (state?.song && player.online) {
        const list = state.queue?.length ? [...state.queue] : [state.song, ...(state.next ?? [])];
        const start = Math.max(0, list.findIndex(item => item.uri === state.song.uri));
        if (state.playing) api('/live/remote/send', 'POST', { id: uuid(), kind: 'toggle' }).catch(() => {});
        playHere(list, start, true).then(() => HERE_ACTS.seek({ ms: position(state) }));
      }
    }),
    h('button', { text: 'Cancel', onclick: () => close() }));
  document.body.append(scrim, panel);
  over();
}

// ---------------------------------------------------------------- the player

/** What the listener just did shows at once and stays until the computer agrees or time's up. */
const HOLD_MS = 4000;
function hold(field, value) {
  player.held.set(field, { value, until: Date.now() + HOLD_MS });
  refreshPlayer();
}
/** What the page shows: the phone's own player while it plays, else the computer's. */
function shown() {
  const state = here.active ? hereState() : player.state ? { ...player.state } : null;
  if (!state) return null;
  for (const [field, { value, until }] of player.held) {
    if (Date.now() < until && state[field] !== value) state[field] = value;
    else player.held.delete(field);
  }
  return state;
}
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
    refreshPlayer();
    return;
  }
  const fresh = state && now - state.at < 30000;
  const wasOnline = player.online;
  player.online = !!fresh;
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

export function buildMini() {
  const line = h('div', { class: 'line' });
  const image = h('div', { class: 'art' });
  const name = h('span', { class: 'name' });
  const by = h('span', { class: 'by' });
  const play = h('button', { class: 'play', 'aria-label': 'Play or pause', onclick: togglePlay });
  const node = h('div', { class: 'mini', hidden: true },
    h('button', { class: 'open', 'aria-label': 'Open what is playing', onclick: openSheet }, image, h('span', { class: 'words' }, name, by)),
    play, line);
  return { node, line, image, name, by, play, uri: null, playing: null, trouble: false };
}

export function refreshPlayer() {
  const state = shown();
  const song = state?.song;
  const mini = player.mini;
  if (mini) {
    mini.node.hidden = !song || !(player.online || player.direct || here.active);
    document.querySelector('main')?.classList.toggle('no-player', mini.node.hidden);
    if (song) {
      if (mini.uri !== song.uri) {
        mini.uri = song.uri;
        markPlaying();
        const image = art(song.image);
        mini.image.replaceWith(image);
        mini.image = image;
        mini.name.textContent = song.name;
        mini.name.className = `name ${sourceOf(song.uri)}`;
        mini.by.textContent = song.artists.join(', ');
      }
      if (mini.playing !== state.playing) {
        mini.playing = state.playing;
        mini.play.replaceChildren(icon(state.playing ? 'pause' : 'play'));
      }
      if (mini.trouble !== !!state.trouble) {
        mini.trouble = !!state.trouble;
        mini.by.textContent = state.trouble ? state.trouble.reason : song.artists.join(', ');
      }
      const at = state.here ? state.position_ms : position(state);
      mini.line.style.width = `${song.duration_ms ? at / song.duration_ms * 100 : 0}%`;
    }
  }
  lockScreenState();
  player.sheet?.update(state);
}
/** Marks the song playing, wherever it's listed on the page. */
export function markPlaying() {
  const uri = shown()?.song?.uri;
  document.querySelectorAll('.row[data-uri]').forEach(row => row.classList.toggle('playing', row.dataset.uri === uri));
}
/** A small knock under the thumb, where the phone can. */
const knock = () => navigator.vibrate?.(8);
/** Moves to `ms` now, and holds it there until the computer agrees. */
function seekTo(ms) {
  if (here.active) { act('seek', { ms }); return; }
  if (!player.state) return;
  player.state.position_ms = ms;
  player.at = Date.now();
  player.sought = { ms, at: Date.now(), until: Date.now() + HOLD_MS, uri: player.state.song?.uri };
  act('seek', { ms });
}
function togglePlay() {
  knock();
  const state = shown();
  hold('playing', !state?.playing);
  act('toggle').catch(() => toast("Couldn't reach Encore."));
}
setInterval(() => { if (!document.hidden) refreshPlayer(); }, 500);

function range(value, max, onchange, label) {
  const input = h('input', { type: 'range', min: 0, max, value, 'aria-label': label });
  const fill = () => input.style.setProperty('--fill', `${(Number(input.value) / (Number(input.max) || 1)) * 100}%`);
  input.addEventListener('input', () => { input.dataset.dragging = '1'; fill(); });
  input.addEventListener('change', () => { delete input.dataset.dragging; onchange(Number(input.value)); });
  fill();
  return { input, fill };
}

function openSheet() {
  if (player.sheet) return;
  over();
  const glow = h('div');
  const cover = h('div', { class: 'cover' });
  const name = h('span', { class: 'big' });
  const artist = h('button', { class: 'artist' });
  const where = h('button', { class: 'where', onclick: targetPicker });
  const heart = h('button', { class: 'heart', 'aria-label': 'Like', onclick: () => {
    const state = shown();
    if (!state?.song) return;
    hold('liked', !state.liked);
    if (here.active) { here.likedUri = state.song.uri; here.liked = !state.liked; }
    act('like', { uri: state.song.uri });
  } }, icon('heart'));
  const seek = range(0, 1000, seekTo, 'Position');
  const elapsed = h('span');
  const total = h('span');
  const main = h('button', { class: 'main', 'aria-label': 'Play or pause', onclick: togglePlay });
  const shuffle = h('button', { class: 'side', 'aria-label': 'Shuffle', onclick: () => { hold('shuffle', !shown()?.shuffle); act('shuffle'); } }, icon('shuffle'));
  const repeat = h('button', { class: 'side', 'aria-label': 'Repeat', onclick: () => act('repeat') }, icon('repeat'));
  const volume = range(50, 100, percent => { hold('volume', percent); act('volume', { percent }); }, 'Volume');
  const trouble = h('div', { class: 'quiet-note', hidden: true });
  const next = h('div', { class: 'upnext' });
  const lyricsBox = h('div', { class: 'lyrics', hidden: true });
  const lyricsButton = h('button', { 'aria-pressed': 'false', text: 'Lyrics', onclick: () => {
    const open = lyricsBox.hidden;
    lyricsBox.hidden = !open;
    lyricsButton.setAttribute('aria-pressed', String(open));
    sheet.querySelector('.cover').hidden = open;
    if (open) readLyrics(shown()?.song?.uri);
  } });
  const shareButton = h('button', { text: 'Share', onclick: () => { const song = shown()?.song; if (song?.link) shareSong(song); else toast("This song can't be shared from here."); } });
  let lyrics = { uri: null, lines: [], at: -1 };
  async function readLyrics(forUri) {
    if (!forUri || lyrics.uri === forUri) return;
    lyrics = { uri: forUri, lines: [], at: -1 };
    lyricsBox.replaceChildren(bones(5));
    try {
      if (!player.online) throw new Error('Lyrics come from Encore on your computer.');
      let found = await ask('lyrics');
      // Just after a change the computer may still hold the last song's words.
      for (let tries = 0; found.uri && found.uri !== forUri && tries < 4; tries++) {
        await new Promise(resolve => setTimeout(resolve, 1500));
        if (lyrics.uri !== forUri) return;
        found = await ask('lyrics');
      }
      if (lyrics.uri !== forUri) return;
      lyrics.lines = found.lines ?? [];
      if (!lyrics.lines.length) {
        lyricsBox.replaceChildren(h('p', { class: 'quiet-note', text: found.instrumental ? 'This one has no words.' : 'No lyrics for this song.' }));
        return;
      }
      lyricsBox.replaceChildren(...lyrics.lines.map(line => (found.synced
        ? h('button', { text: line.text || '♪', onclick: () => {
          seekTo(line.at_ms);
        } })
        : h('p', { text: line.text || ' ' }))));
      lyrics.synced = found.synced;
      lyrics.at = -1;
    } catch (error) {
      if (lyrics.uri === forUri) lyricsBox.replaceChildren(h('p', { class: 'quiet-note', text: error.message }));
    }
  }
  function followLyrics(at) {
    if (!lyrics.synced || lyricsBox.hidden) return;
    let line = -1;
    lyrics.lines.forEach((item, index) => { if (item.at_ms != null && item.at_ms <= at) line = index; });
    if (line === lyrics.at) return;
    lyricsBox.children[lyrics.at]?.classList.remove('now');
    lyrics.at = line;
    const shownLine = lyricsBox.children[line];
    if (shownLine) {
      shownLine.classList.add('now');
      lyricsBox.scrollTop = shownLine.offsetTop - lyricsBox.offsetTop - lyricsBox.clientHeight * 0.35;
    }
  }
  let nextKey = '';
  let uri = null;
  let shownTrouble = '';
  const close = () => { player.sheet = null; sheet.remove(); overClosed(); };
  const sheet = h('div', { class: 'sheet', role: 'dialog', 'aria-label': 'Now playing' },
    glow,
    h('header', {}, h('button', { 'aria-label': 'Close', onclick: close }, icon('down')), where, h('span', { class: 'spacer' })),
    cover,
    lyricsBox,
    h('div', { class: 'heading' }, h('div', {}, name, artist), heart),
    trouble,
    h('div', { class: 'seek' }, seek.input, h('div', { class: 'times' }, elapsed, total)),
    h('div', { class: 'transport' }, shuffle,
      h('button', { 'aria-label': 'Previous', onclick: () => { knock(); act('previous'); } }, icon('previous')),
      main,
      h('button', { 'aria-label': 'Next', onclick: () => { knock(); act('next'); } }, icon('next')),
      repeat),
    h('div', { class: 'volume' }, icon('volume'), volume.input),
    h('div', { class: 'tools' }, lyricsButton, shareButton),
    next);
  // Pulled down from the top, it follows the finger and closes.
  let pull = null;
  sheet.addEventListener('touchstart', event => { pull = sheet.scrollTop <= 0 && event.target.type !== 'range' ? event.touches[0].clientY : null; }, { passive: true });
  sheet.addEventListener('touchmove', event => {
    if (pull === null) return;
    const moved = event.touches[0].clientY - pull;
    if (moved > 0) sheet.style.transform = `translateY(${moved}px)`;
  }, { passive: true });
  const release = event => {
    if (pull === null) return;
    const moved = event.changedTouches[0].clientY - pull;
    pull = null;
    if (moved > 120) close();
    else sheet.style.transform = '';
  };
  sheet.addEventListener('touchend', release);
  sheet.addEventListener('touchcancel', () => { pull = null; sheet.style.transform = ''; });
  document.body.append(sheet);
  let wasPlaying = null;
  player.sheet = { close, update(state) {
    const song = state?.song;
    if (!song) return;
    if (song.uri !== uri) {
      uri = song.uri;
      const image = art(song.image, 'cover');
      image.hidden = !lyricsBox.hidden;
      sheet.querySelector('.cover').replaceWith(image);
      if (!lyricsBox.hidden) readLyrics(song.uri);
      sheet.querySelector('.glow')?.remove();
      if (safeImage(song.image)) sheet.prepend(h('img', { class: 'glow', src: song.image, alt: '' }));
      name.textContent = song.name;
      name.className = `big ${sourceOf(song.uri)}`;
      artist.textContent = song.artists.join(', ');
      artist.onclick = song.artist_id ? () => { close(); go({ page: 'artist', id: song.artist_id }); } : null;
    }
    where.textContent = `${state.device ? `Playing on ${state.device}` : 'Playing on your computer'} ▾`;
    // Only a song the computer said has no audio stops here: Skip, or hand it back.
    const troubleKey = state.trouble ? `${state.trouble.uri}|${state.trouble.reason}` : '';
    if (troubleKey !== shownTrouble) {
      shownTrouble = troubleKey;
      trouble.hidden = !state.trouble;
      trouble.replaceChildren(...(state.trouble ? [
        h('p', { text: state.trouble.reason }),
        h('button', { class: 'play-all second', text: 'Skip', onclick: () => { here.trouble = null; HERE_ACTS.next(); } }),
        h('button', { class: 'play-all second', text: 'Play on computer', onclick: () => playOnComputer(song) }),
      ] : []));
    }
    heart.setAttribute('aria-pressed', String(!!state.liked));
    shuffle.setAttribute('aria-pressed', String(!!state.shuffle));
    repeat.setAttribute('aria-pressed', String(state.repeat !== 'off'));
    repeat.classList.toggle('one', state.repeat === 'one');
    repeat.setAttribute('aria-label', state.repeat === 'one' ? 'Repeat one' : state.repeat === 'all' ? 'Repeat all' : 'Repeat');
    if (wasPlaying !== state.playing) {
      wasPlaying = state.playing;
      main.replaceChildren(icon(state.playing ? 'pause' : 'play'));
    }
    seek.input.max = song.duration_ms || 1;
    const at = state.here ? state.position_ms : position(state);
    followLyrics(at);
    if (!seek.input.dataset.dragging) { seek.input.value = at; seek.fill(); }
    elapsed.textContent = clock(seek.input.dataset.dragging ? Number(seek.input.value) : at);
    total.textContent = clock(song.duration_ms);
    if (!volume.input.dataset.dragging) { volume.input.value = state.volume ?? 0; volume.fill(); }
    const key = (state.next ?? []).map(item => item.uri).join();
    if (key !== nextKey) {
      nextKey = key;
      next.replaceChildren(...(state.next?.length ? [h('h2', { class: 'section', text: 'Up next' }), ...state.next.map(item => row(item, [item], 0))] : []));
    }
  } };
  refreshPlayer();
}

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
hooks.playFrom = playFrom;
hooks.shown = shown;
hooks.pollSpotify = pollSpotify;
setEdgeSinks(applyState, controlHere);

// iOS only lets sound begin inside a tap, so the element is here from the start.
hereAudio();

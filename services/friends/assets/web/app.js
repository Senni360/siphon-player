// Encore on a phone. It's another device of the listener's group account, and Encore
// on their computer answers for the library and plays the sound. With their own
// Spotify app signed in here as well, Spotify carries on with the computer off; that
// sign-in stays in this browser. Everything shown goes in as text, never as markup.
'use strict';

const STORE = { secret: 'encore.secret', name: 'encore.name', group: 'encore.group', client: 'encore.client', spotify: 'encore.spotify' };
const app = document.getElementById('app');

// ---------------------------------------------------------------- small helpers
function h(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === undefined || value === null || value === false) continue;
    if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
    else if (key === 'class') node.className = value;
    else if (key === 'text') node.textContent = value;
    else node.setAttribute(key, value === true ? '' : value);
  }
  node.append(...children.flat(Infinity).filter(child => child !== null && child !== undefined && child !== false));
  return node;
}
const ICONS = {
  home: '<path d="M3 11l9-8 9 8v9a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/>',
  library: '<path d="M5 4v16M10 4v16M15 4l5 16"/>',
  group: '<circle cx="9" cy="8" r="4"/><path d="M2 21c0-4 3-7 7-7s7 3 7 7M16 4a4 4 0 0 1 0 8M22 21c0-3-2-6-5-6.5"/>',
  back: '<path d="M15 5l-7 7 7 7"/>',
  down: '<path d="M5 9l7 7 7-7"/>',
  more: '<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',
  heart: '<path d="M12 20s-8-5-8-11a4.5 4.5 0 0 1 8-3 4.5 4.5 0 0 1 8 3c0 6-8 11-8 11z"/>',
  shuffle: '<path d="M3 6h3c5 0 7 12 12 12h3M3 18h3c2 0 3.500-2 5-4.500M21 6h-3c-2 0-3.500 2-5 4.500M18 3l3 3-3 3M18 15l3 3-3 3"/>',
  repeat: '<path d="M17 3l4 4-4 4M3 11V9a2 2 0 0 1 2-2h16M7 21l-4-4 4-4M21 13v2a2 2 0 0 1-2 2H3"/>',
  volume: '<path d="M4 9v6h4l5 4V5L8 9zM17 9a4 4 0 0 1 0 6"/>',
  reply: '<path d="M21 12a8 8 0 0 1-12 7l-5 1 1-5a8 8 0 1 1 16-3z"/>',
};
const SOLID = {
  play: '<path d="M7 4l14 8-14 8z"/>',
  pause: '<rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/>',
  next: '<path d="M5 4l11 8-11 8zM18 4h2v16h-2z"/>',
  previous: '<path d="M19 4L8 12l11 8zM4 4h2v16H4z"/>',
};
function icon(name) {
  const solid = name in SOLID;
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  if (solid) svg.setAttribute('class', 'solid');
  svg.innerHTML = solid ? SOLID[name] : ICONS[name];
  return svg;
}
const safeImage = url => (typeof url === 'string' && url.startsWith('https://') ? url : null);
function art(url, className = 'art') {
  const safe = safeImage(url);
  return safe ? h('img', { class: className, src: safe, alt: '', loading: 'lazy', decoding: 'async' }) : h('div', { class: className });
}
const sourceOf = uri => (String(uri).startsWith('soundcloud:') ? 'soundcloud' : String(uri).startsWith('youtube:') ? 'youtube' : 'spotify');
const SOURCE_NAMES = { spotify: 'Spotify', soundcloud: 'SoundCloud', youtube: 'YouTube' };
const clock = ms => {
  const seconds = Math.max(0, Math.floor((ms || 0) / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
};
const uuid = () => crypto.randomUUID();
const count = value => new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 }).format(value);

let toastTimer;
function toast(message) {
  document.querySelector('.toast')?.remove();
  document.body.append(h('div', { class: 'toast', role: 'status', text: message }));
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => document.querySelector('.toast')?.remove(), 2600);
}

// ---------------------------------------------------------------- the service
class SignedOut extends Error {}
async function api(path, method = 'GET', body) {
  const secret = localStorage.getItem(STORE.secret);
  const group = localStorage.getItem(STORE.group);
  const answer = await fetch(`/api${path}`, {
    method,
    headers: {
      ...(body !== undefined ? { 'content-type': 'application/json' } : {}),
      ...(secret ? { authorization: `Bearer ${secret}` } : {}),
      ...(group ? { 'x-siphon-group': group } : {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (answer.status === 401 && secret) {
    localStorage.removeItem(STORE.secret);
    throw new SignedOut();
  }
  const value = await answer.json().catch(() => ({}));
  if (!answer.ok) throw new Error(value.error || 'Something went wrong.');
  return value;
}

// ---------------------------------------------------------------- Spotify itself
const SPOTIFY_SCOPES = 'user-read-private user-read-email playlist-read-private playlist-read-collaborative user-library-read user-library-modify user-read-playback-state user-modify-playback-state user-read-currently-playing user-top-read';
const redirectUri = () => `${location.origin}/callback`;
const base64url = bytes => btoa(String.fromCharCode(...new Uint8Array(bytes))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
const hasSpotify = () => !!localStorage.getItem(STORE.spotify);

async function spotifySignIn(clientId) {
  const verifier = base64url(crypto.getRandomValues(new Uint8Array(48)));
  const state = base64url(crypto.getRandomValues(new Uint8Array(16)));
  const challenge = base64url(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(verifier)));
  localStorage.setItem(STORE.client, clientId);
  sessionStorage.setItem('encore.verifier', verifier);
  sessionStorage.setItem('encore.state', state);
  location.href = `https://accounts.spotify.com/authorize?${new URLSearchParams({
    client_id: clientId, response_type: 'code', redirect_uri: redirectUri(), scope: SPOTIFY_SCOPES,
    code_challenge_method: 'S256', code_challenge: challenge, state,
  })}`;
}

async function spotifyTokens(form) {
  const answer = await fetch('https://accounts.spotify.com/api/token', {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ client_id: localStorage.getItem(STORE.client) ?? '', ...form }),
  });
  if (!answer.ok) throw new Error("Spotify didn't accept that. Check your Client ID and redirect address.");
  const tokens = await answer.json();
  const held = JSON.parse(localStorage.getItem(STORE.spotify) ?? '{}');
  const saved = { access: tokens.access_token, refresh: tokens.refresh_token ?? held.refresh, until: Date.now() + tokens.expires_in * 1000 };
  localStorage.setItem(STORE.spotify, JSON.stringify(saved));
  return saved;
}

/** Back from Spotify's sign-in: this proves who the listener is, for their same account. */
async function spotifyReturn() {
  const params = new URLSearchParams(location.search);
  const state = sessionStorage.getItem('encore.state');
  const verifier = sessionStorage.getItem('encore.verifier');
  sessionStorage.removeItem('encore.state');
  sessionStorage.removeItem('encore.verifier');
  history.replaceState(null, '', '/');
  if (params.get('error')) throw new Error('Spotify sign-in was cancelled.');
  if (!params.get('code') || !state || params.get('state') !== state || !verifier) throw new Error("That sign-in didn't match. Try again.");
  const tokens = await spotifyTokens({ grant_type: 'authorization_code', code: params.get('code'), redirect_uri: redirectUri(), code_verifier: verifier });
  const joined = await api('/auth/spotify', 'POST', { token: tokens.access, device: phoneName() });
  localStorage.setItem(STORE.secret, joined.secret);
  localStorage.setItem(STORE.name, joined.display_name);
}

async function spotifyAccess() {
  let saved = JSON.parse(localStorage.getItem(STORE.spotify) ?? 'null');
  if (!saved) throw new Error('Connect Spotify first.');
  if (Date.now() > saved.until - 60000) saved = await spotifyTokens({ grant_type: 'refresh_token', refresh_token: saved.refresh });
  return saved.access;
}

async function web(path, method = 'GET', body) {
  const answer = await fetch(`https://api.spotify.com/v1${path}`, {
    method,
    headers: { authorization: `Bearer ${await spotifyAccess()}`, ...(body !== undefined ? { 'content-type': 'application/json' } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (answer.status === 204 || answer.status === 202) return null;
  if (answer.status === 429) throw new Error('Spotify is busy. Try again in a moment.');
  if (answer.status === 403 && method !== 'GET') throw new Error('Spotify only lets Premium accounts control playback from here.');
  if (!answer.ok) throw new Error("Spotify couldn't do that.");
  return answer.status === 200 ? answer.json().catch(() => null) : null;
}

const spotifySong = track => ({
  uri: track.uri,
  name: track.name,
  artists: (track.artists ?? []).map(artist => artist.name),
  artist_id: track.artists?.[0]?.id,
  image: track.album?.images?.[1]?.url ?? track.album?.images?.[0]?.url,
  duration_ms: track.duration_ms,
});
const playable = item => item && item.type === 'track' && item.uri?.startsWith('spotify:track:');

/** The same questions the computer answers, for Spotify alone. */
const DIRECT = {
  home: async () => ({ picks: (await web('/me/top/tracks?limit=20&time_range=short_term')).items.filter(playable).map(spotifySong), following: [] }),
  library: async () => ({ direct: true, playlists: (await web('/me/playlists?limit=50')).items.filter(Boolean).map(list => ({ id: list.id, uri: list.uri, name: list.name, image: list.images?.[0]?.url })) }),
  liked: async () => ({ name: 'Liked Songs', songs: (await web('/me/tracks?limit=50')).items.map(saved => saved.track).filter(playable).map(spotifySong) }),
  playlist: async ({ target }) => {
    const [about, rows] = await Promise.all([web(`/playlists/${encodeURIComponent(target)}?fields=name,uri`), web(`/playlists/${encodeURIComponent(target)}/tracks?limit=100`)]);
    return { name: about.name, uri: about.uri, songs: rows.items.map(row => row.track).filter(playable).map(spotifySong) };
  },
  album: async ({ target }) => {
    const album = await web(`/albums/${encodeURIComponent(target)}`);
    return { name: album.name, uri: album.uri, songs: album.tracks.items.map(track => spotifySong({ ...track, type: 'track', album })) };
  },
  artist: async ({ target }) => {
    const id = encodeURIComponent(target);
    const [artist, albums] = await Promise.all([web(`/artists/${id}`), web(`/artists/${id}/albums?include_groups=album,single&limit=20`)]);
    const popular = await web(`/artists/${id}/top-tracks`).catch(() => ({ tracks: [] }));
    return {
      name: artist.name, uri: artist.uri, image: artist.images?.[0]?.url, followers: artist.followers?.total,
      popular: popular.tracks.filter(playable).map(spotifySong),
      albums: albums.items.map(album => ({ id: album.id, name: album.name, year: album.release_date?.slice(0, 4), image: album.images?.[1]?.url ?? album.images?.[0]?.url })),
      looking: false,
    };
  },
  search: async ({ query }) => {
    const found = await web(`/search?${new URLSearchParams({ q: query, type: 'track,artist', limit: '20' })}`);
    return {
      artists: found.artists.items.slice(0, 4).map(artist => ({ id: artist.id, name: artist.name, image: artist.images?.[1]?.url ?? artist.images?.[0]?.url })),
      spotify: found.tracks.items.filter(playable).map(spotifySong),
    };
  },
};

/** The buttons, on whichever of the listener's Spotify devices is awake. */
const trackId = uri => uri.split(':').pop();
const DIRECT_ACTS = {
  // What Spotify last said, not what's already shown for this very press.
  toggle: () => web(`/me/player/${player.state?.playing ? 'pause' : 'play'}`, 'PUT'),
  next: () => web('/me/player/next', 'POST'),
  previous: () => web('/me/player/previous', 'POST'),
  seek: ({ ms }) => web(`/me/player/seek?position_ms=${Math.round(ms)}`, 'PUT'),
  volume: ({ percent }) => web(`/me/player/volume?volume_percent=${percent}`, 'PUT'),
  shuffle: () => web(`/me/player/shuffle?state=${!player.state?.shuffle}`, 'PUT'),
  repeat: () => web(`/me/player/repeat?state=${player.state?.repeat === 'off' ? 'context' : 'off'}`, 'PUT'),
  like: async ({ uri }) => {
    const id = encodeURIComponent(trackId(uri));
    const [saved] = await web(`/me/tracks/contains?ids=${id}`);
    await web(`/me/tracks?ids=${id}`, saved ? 'DELETE' : 'PUT');
  },
  queue: ({ uri }) => web(`/me/player/queue?uri=${encodeURIComponent(uri)}`, 'POST'),
  play: async ({ uris, index }) => {
    const songs = uris.filter(uri => uri.startsWith('spotify:'));
    const wanted = uris[index];
    if (!songs.includes(wanted)) throw new Error('That song plays from Encore on your computer.');
    const { devices } = await web('/me/player/devices');
    const device = devices.find(item => item.is_active) ?? devices[0];
    // Nothing awake to send it to: the Spotify app takes it from here.
    if (!device) { location.href = `https://open.spotify.com/track/${trackId(wanted)}`; return; }
    await web(`/me/player/play?device_id=${encodeURIComponent(device.id)}`, 'PUT', { uris: songs, offset: { uri: wanted } });
  },
};

/** Tells the computer to do something, or Spotify when the computer's not there. */
async function act(kind, args = {}) {
  player.lastAct = Date.now();
  // Playing on this phone, the buttons work the phone's own player.
  if (here.active && HERE_ACTS[kind]) return HERE_ACTS[kind](args);
  if (!player.online && hasSpotify() && DIRECT_ACTS[kind]) {
    try {
      await DIRECT_ACTS[kind](args);
      setTimeout(pollSpotify, 500);
    } catch (error) { toast(error.message); }
    return;
  }
  await api('/live/remote/send', 'POST', { id: uuid(), kind, ...args });
}

/** Asks the computer something, and waits for it to answer. */
async function ask(kind, args = {}, patience = 45000) {
  if (!player.online && hasSpotify() && DIRECT[kind]) return DIRECT[kind](args);
  // Past the time the computer takes to notice, it isn't coming: say so straight away.
  if (!player.online && Date.now() - openedAt > WAKE_MS) throw new Error("Encore isn't open on your computer.");
  const id = uuid();
  await api('/live/remote/send', 'POST', { id, kind, ...args });
  const until = Date.now() + patience;
  while (Date.now() < until) {
    await new Promise(resolve => setTimeout(resolve, 600));
    const answer = await api(`/live/remote/reply/${id}`);
    if (answer.ready) {
      if (answer.reply.error) throw new Error(answer.reply.error);
      return answer.reply;
    }
  }
  throw new Error(player.online ? 'Encore took too long to answer.' : "Encore isn't open on your computer.");
}

// ---------------------------------------------------------------- playing on this phone
// Every song plays in this page; the computer only says where its audio is. A Spotify
// song plays as the same song from YouTube or SoundCloud, which the computer finds, so
// a mixed playlist runs straight through. Only when there's no such match does it go
// to the Spotify app, and the list carries on when it ends there.
const TARGET_KEY = 'encore.target';
const here = { audio: null, list: [], index: -1, spotify: null, active: false };
const playsHere = () => localStorage.getItem(TARGET_KEY) === 'phone';

function hereAudio() {
  if (here.audio) return here.audio;
  const audio = new Audio();
  audio.preload = 'auto';
  audio.addEventListener('ended', () => playIndex(here.index + 1));
  audio.addEventListener('error', () => {
    if (!here.active || here.spotify || !audio.src) return;
    const song = here.list[here.index];
    toast(`${song?.name ?? 'That song'} can't play on this phone.`);
    playIndex(here.index + 1);
  });
  for (const event of ['play', 'pause', 'loadedmetadata']) audio.addEventListener(event, refreshPlayer);
  here.audio = audio;
  return audio;
}

async function playHere(list, index) {
  here.list = list;
  here.active = true;
  await playIndex(index);
}

function stopHere() {
  here.audio?.pause();
  here.active = false;
  here.spotify = null;
  refreshPlayer();
}

async function playIndex(index) {
  if (index < 0 || index >= here.list.length) {
    here.audio?.pause();
    here.spotify = null;
    refreshPlayer();
    return;
  }
  here.index = index;
  here.spotify = null;
  const song = here.list[index];
  mediaSession(song);
  refreshPlayer();
  markPlaying();
  try {
    const found = await streamOf(song);
    if (here.index !== index) return;
    // The next song is looked up while this one plays, so there's no gap between them.
    const next = here.list[index + 1];
    if (next) streamOf(next).catch(() => {});
    const audio = hereAudio();
    if (found.hls && !audio.canPlayType('application/vnd.apple.mpegurl')) throw new Error("this browser can't play it");
    audio.src = found.url;
    await audio.play();
  } catch (error) {
    if (here.index !== index) return;
    if (song.uri.startsWith('spotify:track:')) return handOff(song, index);
    toast(`${song.name}: ${error.message}`);
    playIndex(index + 1);
  }
}

/** Where a song's audio is, asked once per song for a while. */
const STREAM_KEEP_MS = 20 * 60 * 1000;
const streams = new Map();
function streamOf(song) {
  const held = streams.get(song.uri);
  if (held && Date.now() - held.at < STREAM_KEEP_MS) return held.found;
  const question = song.uri.startsWith('spotify:')
    ? { uri: song.uri, name: song.name, artists: song.artists, duration_ms: song.duration_ms }
    : { uri: song.uri };
  const found = ask('stream', question);
  streams.set(song.uri, { at: Date.now(), found });
  found.catch(() => streams.delete(song.uri));
  return found;
}

/** A Spotify song plays in the Spotify app on this phone, and the list waits for it. */
async function handOff(song, index) {
  here.audio?.pause();
  const app = `https://open.spotify.com/track/${trackId(song.uri)}`;
  if (!hasSpotify()) {
    toast('Spotify songs play in the Spotify app.');
    location.href = app;
    stopHere();
    return;
  }
  try {
    const { devices } = await web('/me/player/devices');
    const device = devices.find(item => item.type === 'Smartphone') ?? devices.find(item => item.is_active) ?? devices[0];
    if (!device) { location.href = app; stopHere(); return; }
    await web(`/me/player/play?device_id=${encodeURIComponent(device.id)}`, 'PUT', { uris: [song.uri] });
    here.spotify = { uri: song.uri, position: 0, playing: true, at: Date.now(), started: Date.now(), seen: false };
    followSpotify(index, song);
  } catch (error) {
    toast(error.message);
    playIndex(index + 1);
  }
}

/** Watches the Spotify app until its song ends, then carries on with the list here. */
function followSpotify(index, song) {
  const look = async () => {
    if (here.index !== index || !here.spotify) return;
    try {
      const now = await web('/me/player');
      const same = now?.item?.uri === here.spotify.uri;
      if (same) {
        Object.assign(here.spotify, { position: now.progress_ms ?? 0, playing: now.is_playing, at: Date.now(), seen: true });
        refreshPlayer();
        const length = now.item.duration_ms ?? song.duration_ms ?? 0;
        const finished = !now.is_playing && ((now.progress_ms ?? 0) >= length - 3000
          || ((now.progress_ms ?? 0) === 0 && Date.now() - here.spotify.started > length - 5000));
        if (finished) return playIndex(index + 1);
      } else if (here.spotify.seen) {
        // Spotify moved on by itself: take the list back.
        await web('/me/player/pause', 'PUT').catch(() => {});
        return playIndex(index + 1);
      }
    } catch { /* the next look tries again */ }
    setTimeout(look, 2000);
  };
  setTimeout(look, 2500);
}

/** The phone's own playback, in the shape the player shows. */
function hereState() {
  const song = here.list[here.index];
  if (!song) return null;
  const audio = here.audio;
  const spotify = here.spotify;
  const playing = spotify ? spotify.playing : !!audio && !audio.paused;
  const position = spotify ? spotify.position + (spotify.playing ? Date.now() - spotify.at : 0) : (audio?.currentTime ?? 0) * 1000;
  return {
    song: { ...song, duration_ms: song.duration_ms || (audio?.duration ? audio.duration * 1000 : 0) },
    position_ms: position, playing, liked: false, shuffle: false, repeat: 'off',
    volume: Math.round((audio?.volume ?? 1) * 100), device: spotify ? 'Spotify on this phone' : 'This phone',
    next: here.list.slice(here.index + 1, here.index + 6), here: true,
  };
}

const HERE_ACTS = {
  toggle: () => {
    if (here.spotify) return web(`/me/player/${here.spotify.playing ? 'pause' : 'play'}`, 'PUT').then(() => { here.spotify.playing = !here.spotify.playing; here.spotify.at = Date.now(); refreshPlayer(); });
    const audio = hereAudio();
    return audio.paused ? audio.play() : audio.pause();
  },
  next: () => playIndex(here.index + 1),
  previous: () => {
    const at = hereState()?.position_ms ?? 0;
    if (at > 3000) return HERE_ACTS.seek({ ms: 0 });
    return playIndex(Math.max(0, here.index - 1));
  },
  seek: ({ ms }) => {
    if (here.spotify) return web(`/me/player/seek?position_ms=${Math.round(ms)}`, 'PUT').then(() => { here.spotify.position = ms; here.spotify.at = Date.now(); });
    hereAudio().currentTime = ms / 1000;
  },
  volume: ({ percent }) => { hereAudio().volume = Math.min(1, Math.max(0, percent / 100)); },
  shuffle: () => toast("Shuffle isn't on this phone yet."),
  repeat: () => toast("Repeat isn't on this phone yet."),
};

/** The lock screen and headphones show the song and work the buttons. */
function mediaSession(song) {
  if (!('mediaSession' in navigator)) return;
  const image = safeImage(song.image);
  navigator.mediaSession.metadata = new MediaMetadata({ title: song.name, artist: song.artists.join(', '), artwork: image ? [{ src: image, sizes: '300x300' }] : [] });
  const handlers = {
    play: () => HERE_ACTS.toggle(), pause: () => HERE_ACTS.toggle(),
    nexttrack: () => HERE_ACTS.next(), previoustrack: () => HERE_ACTS.previous(),
    seekto: details => HERE_ACTS.seek({ ms: details.seekTime * 1000 }),
  };
  for (const [action, handler] of Object.entries(handlers)) {
    try { navigator.mediaSession.setActionHandler(action, handler); } catch { /* not every phone has every one */ }
  }
}

/** Where the sound comes from: the computer or this phone. */
function targetPicker() {
  const scrim = h('div', { class: 'scrim', onclick: () => close() });
  const close = () => { scrim.remove(); panel.remove(); };
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
      // Whatever the computer was playing carries on here, from the same place.
      const state = player.state;
      if (state?.song && player.online) {
        const list = [state.song, ...(state.next ?? [])];
        if (state.playing) api('/live/remote/send', 'POST', { id: uuid(), kind: 'toggle' }).catch(() => {});
        playHere(list, 0).then(() => {
          if (!here.spotify) HERE_ACTS.seek({ ms: position(state) });
        });
      }
      refreshPlayer();
    }),
    player.direct || hasSpotify() ? choose('A Spotify device…', false, () => devicePicker()) : null,
    h('button', { text: 'Cancel', onclick: () => close() }));
  document.body.append(scrim, panel);
}

// ---------------------------------------------------------------- the player
const player = { state: null, online: false, direct: false, at: 0, held: new Map(), lastAct: 0, sheet: null, mini: null };

/** What the listener just did shows at once and stays until the computer agrees or time's up. */
const HOLD_MS = 4000;
function hold(field, value) {
  player.held.set(field, { value, until: Date.now() + HOLD_MS });
  refreshPlayer();
}
function shown() {
  if (here.active) return hereState();
  if (!player.state) return null;
  const merged = { ...player.state };
  for (const [field, { value, until }] of player.held) {
    if (Date.now() < until && player.state[field] !== value) merged[field] = value;
    else player.held.delete(field);
  }
  return merged;
}
function position(state) {
  if (!state) return 0;
  const moved = state.playing ? Date.now() - player.at : 0;
  return Math.min(state.position_ms + moved, state.song?.duration_ms ?? Infinity);
}

async function pollState() {
  if (!document.hidden && localStorage.getItem(STORE.secret)) {
    try {
      const { state, now } = await api('/live/remote/state');
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
        renderBanner();
        // Only Spotify's own answers differ from the computer's, so nothing else reloads.
        if (booted && hasSpotify()) { tabViews.clear(); render(true); }
      }
      booted = true;
      refreshPlayer();
    } catch (error) {
      if (error instanceof SignedOut) { start(); return; }
    }
  }
  setTimeout(pollState, player.online || !hasSpotify() ? 1200 : 3000);
}
let booted = false;

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

function buildMini() {
  const line = h('div', { class: 'line' });
  const image = h('div', { class: 'art' });
  const name = h('span', { class: 'name' });
  const by = h('span', { class: 'by' });
  const play = h('button', { class: 'play', 'aria-label': 'Play or pause', onclick: togglePlay });
  const node = h('div', { class: 'mini', hidden: true },
    h('button', { class: 'open', 'aria-label': 'Open what is playing', onclick: openSheet }, image, h('span', { class: 'words' }, name, by)),
    play, line);
  return { node, line, image, name, by, play, uri: null, playing: null };
}

function refreshPlayer() {
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
      const at = state.here ? state.position_ms : position(state);
      mini.line.style.width = `${song.duration_ms ? at / song.duration_ms * 100 : 0}%`;
    }
  }
  player.sheet?.update(state);
}
/** Marks the song playing, wherever it's listed on the page. */
function markPlaying() {
  const uri = player.state?.song?.uri;
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
  history.pushState(null, '');
  const glow = h('div');
  const cover = h('div', { class: 'cover' });
  const name = h('span', { class: 'big' });
  const artist = h('button', { class: 'artist' });
  const where = h('button', { class: 'where', onclick: targetPicker });
  const heart = h('button', { class: 'heart', 'aria-label': 'Like', onclick: () => {
    const state = shown();
    if (!state?.song) return;
    hold('liked', !state.liked);
    act('like', { uri: state.song.uri });
  } }, icon('heart'));
  const seek = range(0, 1000, seekTo, 'Position');
  const elapsed = h('span');
  const total = h('span');
  const main = h('button', { class: 'main', 'aria-label': 'Play or pause', onclick: togglePlay });
  const shuffle = h('button', { class: 'side', 'aria-label': 'Shuffle', onclick: () => { hold('shuffle', !shown()?.shuffle); act('shuffle'); } }, icon('shuffle'));
  const repeat = h('button', { class: 'side', 'aria-label': 'Repeat', onclick: () => act('repeat') }, icon('repeat'));
  const volume = range(50, 100, percent => { hold('volume', percent); act('volume', { percent }); }, 'Volume');
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
  const close = () => { player.sheet = null; sheet.remove(); };
  const sheet = h('div', { class: 'sheet', role: 'dialog', 'aria-label': 'Now playing' },
    glow,
    h('header', {}, h('button', { 'aria-label': 'Close', onclick: close }, icon('down')), where, h('span', { class: 'spacer' })),
    cover,
    lyricsBox,
    h('div', { class: 'heading' }, h('div', {}, name, artist), heart),
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

/** Which of the listener's Spotify devices the sound comes from, with the computer off. */
async function devicePicker() {
  let devices;
  try {
    ({ devices } = await web('/me/player/devices'));
  } catch (error) { toast(error.message); return; }
  const scrim = h('div', { class: 'scrim', onclick: () => close() });
  const close = () => { scrim.remove(); panel.remove(); };
  const panel = h('div', { class: 'actions', role: 'menu' },
    h('div', { class: 'about' }, h('b', { text: 'Play on' }), h('span', { text: devices.length ? 'Your Spotify devices' : 'Open Spotify on a device, then look again.' })),
    devices.map(device => h('button', { text: `${device.is_active ? '♪ ' : ''}${device.name}`, onclick: async () => {
      close();
      try {
        await web('/me/player', 'PUT', { device_ids: [device.id], play: true });
        setTimeout(pollSpotify, 700);
      } catch (error) { toast(error.message); }
    } })),
    h('button', { text: 'Cancel', onclick: () => close() }));
  document.body.append(scrim, panel);
}

// ---------------------------------------------------------------- rows and actions
function songActions(song) {
  const scrim = h('div', { class: 'scrim', onclick: () => close() });
  const close = () => { scrim.remove(); panel.remove(); };
  const choose = (label, run) => h('button', { onclick: () => { close(); run(); } , text: label });
  const panel = h('div', { class: 'actions', role: 'menu' },
    h('div', { class: 'about' }, h('b', { text: song.name, class: sourceOf(song.uri) }), h('span', { text: song.artists.join(', ') })),
    choose('Play next', () => act('queue', { uri: song.uri }).then(() => toast('Added to the queue'))),
    choose('Like', () => act('like', { uri: song.uri }).then(() => toast('Done'))),
    song.link && choose('Share with the group', () => shareSong(song)),
    song.artist_id && choose('Go to artist', () => go({ page: 'artist', id: song.artist_id })),
    choose('Cancel', () => {}));
  document.body.append(scrim, panel);
}
/** Shares a song with the group, with a note if there's something to say. */
function shareSong(song) {
  const note = h('input', { type: 'text', placeholder: 'Say something (optional)', 'aria-label': 'Note', maxlength: 280, enterkeyhint: 'send' });
  const scrim = h('div', { class: 'scrim', onclick: () => close() });
  const close = () => { scrim.remove(); panel.remove(); };
  const panel = h('form', { class: 'actions share', onsubmit: async event => {
    event.preventDefault();
    close();
    try {
      await api('/posts', 'POST', { url: song.link, source: sourceOf(song.uri), title: song.name, artists: song.artists.slice(0, 20), artwork_url: safeImage(song.image), duration_ms: song.duration_ms ?? 0, note: note.value.trim() });
      tabViews.delete('group');
      toast('Shared with the group');
    } catch (error) { toast(error.message); }
  } },
    h('div', { class: 'about' }, h('b', { text: song.name, class: sourceOf(song.uri) }), h('span', { text: song.artists.join(', ') })),
    note,
    h('button', { class: 'send-share', type: 'submit', text: 'Share with the group' }));
  document.body.append(scrim, panel);
  note.focus();
}

function row(song, list, index) {
  const playing = player.state?.song?.uri === song.uri;
  const by = song.artists.join(', ');
  const plays = song.plays ? ` · ${count(song.plays)} plays` : '';
  return h('div', { class: `row${playing ? ' playing' : ''}`, 'data-uri': song.uri },
    art(song.image),
    h('button', { class: 'words', onclick: () => playFrom(list, index) },
      h('span', { class: `name ${sourceOf(song.uri)}`, text: song.name }),
      h('span', { class: 'by', text: by + plays })),
    h('button', { class: 'more', 'aria-label': `More for ${song.name}`, onclick: () => songActions(song) }, icon('more')));
}
function playFrom(list, index) {
  if (playsHere()) { playHere(list, index); return; }
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
function rows(list, limit) {
  const box = h('div');
  const draw = shown => {
    box.replaceChildren(...list.slice(0, shown).map((song, index) => row(song, list, index)));
    if (list.length > shown) box.append(h('button', { class: 'see-more', text: 'See more', onclick: () => draw(shown + 20) }));
  };
  draw(limit ?? list.length);
  return box;
}
function section(title, list, note, limit = 5) {
  if (!list?.length) return null;
  const from = {};
  for (const song of list) from[sourceOf(song.uri)] = (from[sourceOf(song.uri)] ?? 0) + 1;
  const told = note ?? Object.entries(from).map(([source, amount]) => `${amount} on ${SOURCE_NAMES[source]}`).join(' · ');
  return [h('h2', { class: 'section', text: title }), h('p', { class: 'note', text: told }), rows(list, limit)];
}
const bones = amount => h('div', {}, Array.from({ length: amount }, () => h('div', { class: 'bone line' })));
const failed = (error, retry) => h('div', { class: 'empty' }, h('p', { text: error.message }), retry && h('button', { class: 'see-more', text: 'Try again', onclick: retry }));

// ---------------------------------------------------------------- screens
const trail = [];
let current = { page: 'home' };
let screen = null;
let bannerSlot = null;
let drawn = 0;

function go(next, remember = true) {
  keepPlace();
  if (remember) trail.push(current);
  current = next;
  history.pushState(null, '');
  render();
}
function back() {
  keepPlace();
  current = trail.pop() ?? { page: 'home' };
  render();
}
// On a computer's browser, Escape closes what's on top.
addEventListener('keydown', event => {
  if (event.key !== 'Escape') return;
  if (document.querySelector('.actions')) document.querySelector('.scrim')?.click();
  else if (player.sheet) player.sheet.close();
});
addEventListener('popstate', () => {
  if (document.querySelector('.actions')) { document.querySelector('.scrim')?.click(); return; }
  if (player.sheet) player.sheet.close();
  else if (trail.length) back();
});

function backButton() {
  return h('button', { class: 'back', onclick: back, 'aria-label': 'Back' }, icon('back'));
}
/** The computer looks in every so often while nobody's watching, so give it that long. */
const WAKE_MS = 18000;
const openedAt = Date.now();
function renderBanner() {
  if (!player.online && Date.now() - openedAt < WAKE_MS) {
    bannerSlot?.replaceChildren(h('div', { class: 'banner' }, h('div', {}, h('b', { text: 'Connecting to your computer…' }), 'This can take a few seconds.')));
    setTimeout(renderBanner, WAKE_MS);
    return;
  }
  const telling = hasSpotify()
    ? 'Showing your Spotify, played on your Spotify devices. Open it for SoundCloud, YouTube and everything else.'
    : 'Open it to browse your library and play. Your group works from here either way.';
  bannerSlot?.replaceChildren(...(player.online ? [] : [h('div', { class: 'banner' }, h('span', { class: 'dot' }),
    h('div', {}, h('b', { text: "Encore isn't open on your computer" }), telling))]));
}

/** Fills `into`, even if the page has been left meanwhile: going back finds it done. */
async function load(fill, into) {
  const mine = {};
  into.loading = mine;
  const run = async () => {
    into.replaceChildren(bones(6));
    try {
      const content = await fill();
      if (into.loading === mine) into.replaceChildren(...[content].flat(Infinity).filter(Boolean));
    } catch (error) {
      if (error instanceof SignedOut) start();
      else if (into.loading === mine) into.replaceChildren(failed(error, run));
    }
  };
  await run();
}

function card(title, by, image, onclick, uri) {
  return h('button', { class: 'card', onclick }, art(image, ''), h('span', { class: `name${uri ? ` ${sourceOf(uri)}` : ''}`, text: title }), by && h('span', { class: 'by', text: by }));
}

/** Once, a tip for keeping Encore on the home screen, where it opens like an app. */
function installTip() {
  const installed = navigator.standalone || matchMedia('(display-mode: standalone)').matches;
  if (installed || localStorage.getItem('encore.tip')) return null;
  const how = /iPhone|iPad/.test(navigator.userAgent) ? 'Press Share, then Add to Home Screen.' : "Open your browser's menu, then Add to Home screen.";
  const tip = h('div', { class: 'banner' }, h('div', {}, h('b', { text: 'Keep Encore on your home screen' }), how),
    h('button', { class: 'tip-close', 'aria-label': 'Dismiss', onclick: () => { localStorage.setItem('encore.tip', '1'); tip.remove(); } }, '×'));
  return tip;
}

function homeScreen() {
  const body = h('div');
  load(async () => {
    const found = await ask('home');
    const picks = found.picks ?? [];
    const [first, ...rest] = picks;
    if (!first) return h('p', { class: 'empty', text: 'Play some music on Encore and your picks show up here.' });
    return [
      h('button', { class: 'lead', onclick: () => playFrom(picks, 0) }, art(first.image, ''), h('span', { class: 'words' },
        h('span', { class: `name ${sourceOf(first.uri)}`, text: first.name }), h('span', { class: 'by', text: first.artists.join(', ') }))),
      h('h2', { class: 'section', text: 'For you' }),
      h('div', { class: 'cards' }, rest.slice(0, 8).map((song, index) => card(song.name, song.artists.join(', '), song.image, () => playFrom(picks, index + 1), song.uri))),
      found.recent?.length ? [h('h2', { class: 'section', text: 'Jump back in' }), rows(found.recent, 5)] : null,
      found.made_for_you?.length ? [h('h2', { class: 'section', text: 'Made for you' }), h('div', { class: 'cards' }, found.made_for_you.slice(0, 6).map(list => card(list.name, null, list.image, () => go({ page: 'list', kind: 'playlist', id: list.id }))))] : null,
      section('New on SoundCloud from artists you follow', found.news, ''),
      section('New from who you follow', found.following),
    ];
  }, body);
  return [installTip(), h('h1', { class: 'title', text: greeting() }), body];
}
function greeting() {
  const hour = new Date().getHours();
  return hour < 5 ? 'Good night' : hour < 12 ? 'Good morning' : hour < 18 ? 'Good afternoon' : 'Good evening';
}

const RECENT_KEY = 'encore.searches';
const recentSearches = () => { try { return JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]'); } catch { return []; } };
function rememberSearch(query) {
  localStorage.setItem(RECENT_KEY, JSON.stringify([query, ...recentSearches().filter(old => old !== query)].slice(0, 8)));
}

function searchScreen() {
  const results = h('div');
  const input = h('input', { type: 'search', placeholder: 'Spotify, SoundCloud and YouTube', 'aria-label': 'Search', enterkeyhint: 'search', autocapitalize: 'off', autocorrect: 'off', value: current.query ?? '' });
  const run = () => {
    const query = input.value.trim();
    current.query = query;
    if (!query) { showRecent(); return; }
    rememberSearch(query);
    load(async () => {
      const found = await ask('search', { query });
      // Each part says which source it's from, so the chips can show one at a time.
      const part = (source, ...nodes) => h('div', { 'data-source': source }, nodes);
      const parts = [];
      if (found.artists?.length) {
        parts.push(part('spotify', h('h2', { class: 'section', text: 'Artists' }), h('div', { class: 'cards' },
          found.artists.slice(0, 4).map(artist => card(artist.name, 'Artist', artist.image, () => go({ page: 'artist', id: artist.id }))))));
      }
      for (const source of ['soundcloud', 'youtube']) {
        const accounts = (found.accounts ?? []).filter(account => account.source === source);
        if (!accounts.length) continue;
        parts.push(part(source, h('h2', { class: 'section', text: `People on ${SOURCE_NAMES[source]}` }), h('div', { class: 'chips' }, accounts.map(account => h('button', { class: 'chip', onclick: () => go({ page: 'list', kind: 'source', uri: account.uri }) },
          art(account.image, ''), h('span', {}, account.name, h('small', { class: account.source, text: SOURCE_NAMES[account.source] })))))));
      }
      for (const source of ['spotify', 'soundcloud', 'youtube']) {
        const songs = section(`On ${SOURCE_NAMES[source]}`, found[source], '');
        if (songs) parts.push(part(source, songs));
      }
      if (!parts.length) return h('p', { class: 'empty', text: `Nothing found for "${query}".` });
      const shown = new Set(parts.map(node => node.dataset.source));
      const scope = h('div', { class: 'chips scope' });
      const pick = wanted => {
        current.scope = wanted;
        scope.querySelectorAll('button').forEach(button => button.setAttribute('aria-pressed', String(button.dataset.scope === wanted)));
        parts.forEach(node => { node.hidden = wanted !== 'all' && node.dataset.source !== wanted; });
      };
      scope.append(...['all', 'spotify', 'soundcloud', 'youtube'].filter(source => source === 'all' || shown.has(source))
        .map(source => h('button', { class: 'chip plain', 'data-scope': source, text: source === 'all' ? 'All' : SOURCE_NAMES[source], onclick: () => pick(source) })));
      pick(shown.has(current.scope) ? current.scope : 'all');
      return [shown.size > 1 ? scope : null, ...parts];
    }, results);
  };
  const showRecent = () => {
    const recent = recentSearches();
    results.loading = null;
    results.replaceChildren(...(recent.length ? [h('h2', { class: 'section', text: 'Recent' }), h('div', { class: 'chips' }, recent.map(query => h('button', { class: 'chip plain', text: query, onclick: () => { input.value = query; run(); } })))] : []));
  };
  input.addEventListener('input', () => { if (!input.value.trim()) showRecent(); });
  const form = h('form', { class: 'search', role: 'search', onsubmit: event => { event.preventDefault(); input.blur(); run(); } }, icon('search'), input);
  if (current.query) setTimeout(run);
  else showRecent();
  return [h('h1', { class: 'title', text: 'Search' }), form, results];
}

function libraryScreen() {
  const body = h('div');
  const place = (title, image, by, onclick) => h('div', { class: 'row' }, art(image), h('button', { class: 'words', onclick }, h('span', { class: 'name', text: title }), by && h('span', { class: 'by', text: by })));
  load(async () => {
    const found = await ask('library');
    return [
      place('Liked Songs', null, 'Spotify', () => go({ page: 'list', kind: 'liked' })),
      !found.direct && place('SoundCloud likes', null, 'SoundCloud', () => go({ page: 'list', kind: 'soundcloud_likes' })),
      !found.direct && place('YouTube likes', null, 'YouTube', () => go({ page: 'list', kind: 'youtube_likes' })),
      found.mixed?.length ? [h('h2', { class: 'section', text: 'Mixed playlists' }),
        found.mixed.map(list => place(list.name, list.image, `${list.count} songs · Spotify, SoundCloud and YouTube`, () => go({ page: 'list', kind: 'mixed', id: list.id })))] : null,
      h('h2', { class: 'section', text: 'Playlists' }),
      (found.playlists ?? []).map(list => place(list.name, list.image, null, () => go({ page: 'list', kind: 'playlist', id: list.id, uri: list.uri }))),
    ];
  }, body);
  const connect = !hasSpotify() && h('button', { class: 'quiet', text: 'Keep Spotify going with the computer off', onclick: () => spotifyPanel() });
  return [h('h1', { class: 'title', text: 'Library' }), body, h('div', { class: 'settings-link' }, connect, connect && h('br'), h('button', { class: 'quiet', text: 'Unlink this phone', onclick: unlink }))];
}

function shuffled(list) {
  const out = [...list];
  for (let at = out.length - 1; at > 0; at--) {
    const other = Math.floor(Math.random() * (at + 1));
    [out[at], out[other]] = [out[other], out[at]];
  }
  return out;
}

function listScreen() {
  const body = h('div');
  const { kind, id } = current;
  load(async () => {
    const found = await ask(kind, id ? { target: id } : {});
    const songs = found.songs ?? [];
    return [
      h('h1', { class: 'title', text: found.name ?? 'Songs' }),
      songs.length ? h('div', { class: 'list-actions' },
        h('button', { class: 'play-all', onclick: () => playFrom(songs, 0) }, icon('play'), 'Play'),
        h('button', { class: 'play-all second', onclick: () => playFrom(shuffled(songs), 0) }, icon('shuffle'), 'Shuffle')) : h('p', { class: 'empty', text: 'Nothing here yet.' }),
      h('div', { class: 'upnext' }, rows(songs, 50)),
    ];
  }, body);
  return [backButton(), body];
}

/** Follow or unfollow, shown at once and put back if it doesn't go through. */
function followButton(found) {
  const button = h('button', { class: 'play-all second', 'aria-pressed': String(found.following), text: found.following ? 'Following' : 'Follow', onclick: async () => {
    found.following = !found.following;
    button.textContent = found.following ? 'Following' : 'Follow';
    button.setAttribute('aria-pressed', String(found.following));
    try {
      await act('like', { uri: found.uri });
    } catch {
      found.following = !found.following;
      button.textContent = found.following ? 'Following' : 'Follow';
      button.setAttribute('aria-pressed', String(found.following));
      toast("That didn't go through.");
    }
  } });
  return button;
}

function artistScreen() {
  const id = current.id;
  const body = h('div');
  body.append(bones(8));
  let tries = 0;
  const draw = found => {
    const everything = [...(found.popular ?? []), ...(found.not_on_spotify ?? [])];
    body.replaceChildren(...[
      h('div', { class: 'hero' }, safeImage(found.image) && h('img', { src: found.image, alt: '' }), backButton(),
        h('div', {}, h('h1', { text: found.name }), found.followers != null && h('p', { text: [
          `${count(found.followers)} ${found.accounts?.some(account => account.followers) ? 'on Spotify' : 'followers'}`,
          ...(found.accounts ?? []).filter(account => account.followers).map(account => `${count(account.followers)} on ${SOURCE_NAMES[account.source]}`),
        ].join(' · ') }),
          h('div', { class: 'list-actions' },
            everything.length > 0 && h('button', { class: 'play-all', onclick: () => playFrom(everything, 0) }, icon('play'), 'Play everything'),
            found.following != null && followButton(found)))),
      found.accounts?.length ? h('div', { class: 'chips' }, found.accounts.map(account => h('button', { class: 'chip', onclick: () => go({ page: 'list', kind: 'source', uri: account.uri }) },
        art(account.image, ''), h('span', {}, account.name, h('small', { class: account.source, text: SOURCE_NAMES[account.source] }))))) : null,
      section('Popular', found.popular, ''),
      section('Not on Spotify', found.not_on_spotify),
      found.sets?.length ? [h('h2', { class: 'section', text: 'Sets and mixtapes' }), h('p', { class: 'note', text: 'On SoundCloud' }), h('div', { class: 'cards' }, found.sets.slice(0, 8).map(set => card(set.name, set.songs ? `${set.songs} songs` : null, set.image, () => go({ page: 'list', kind: 'source', uri: set.uri }))))] : null,
      found.also_as?.length ? section(`Also posts as ${found.also_as.join(', ')}`, found.alternate) : null,
      found.albums?.length ? [h('h2', { class: 'section', text: 'Discography' }), h('div', { class: 'cards' }, found.albums.slice(0, 8).map(album => card(album.name, album.year, album.image, () => go({ page: 'list', kind: 'album', id: album.id }))))] : null,
      section(`Produced by ${found.name}`, found.produced),
      section('Snippets and unreleased', found.unreleased),
      section('Edits and remixes', found.edits),
      found.looking ? h('p', { class: 'note', text: 'Still looking on SoundCloud and YouTube…' }) : null,
    ].flat(Infinity).filter(Boolean));
  };
  const read = async () => {
    try {
      const found = await ask('artist', { target: id });
      draw(found);
      // What's on the other sources arrives while the page is open.
      if (found.looking && tries++ < 12) setTimeout(read, 3000);
    } catch (error) {
      if (error instanceof SignedOut) start();
      else if (!body.querySelector('.hero')) body.replaceChildren(backButton(), failed(error, () => { body.replaceChildren(bones(8)); read(); }));
    }
  };
  read();
  return [body];
}

function sourceScreen() {
  const body = h('div');
  const { uri } = current;
  load(async () => {
    const found = await ask('source', { uri });
    const songs = found.songs ?? [];
    return [
      found.image ? h('div', { class: 'source-head' }, art(found.image, 'source-picture')) : null,
      h('h1', { class: 'title', text: found.name ?? 'Songs' }),
      h('p', { class: 'note', text: SOURCE_NAMES[sourceOf(uri)] }),
      songs.length ? h('div', { class: 'list-actions' },
        h('button', { class: 'play-all', onclick: () => playFrom(songs, 0) }, icon('play'), 'Play'),
        h('button', { class: 'play-all second', onclick: () => playFrom(shuffled(songs), 0) }, icon('shuffle'), 'Shuffle')) : h('p', { class: 'empty', text: 'Nothing here yet.' }),
      h('div', { class: 'upnext' }, rows(songs, 50)),
    ];
  }, body);
  return [backButton(), body];
}

function playShared(url) {
  // With the computer off, the song's own app can still play its own link.
  if (!player.online) { location.href = url; return; }
  act('play_link', { url }).catch(() => toast("Couldn't reach Encore."));
  toast('Starting on your computer');
}

function postView(post, refresh) {
  const like = h('button', { 'aria-pressed': String(!!post.liked), 'aria-label': 'Heart', onclick: async () => {
    post.liked = !post.liked;
    post.like_count += post.liked ? 1 : -1;
    like.setAttribute('aria-pressed', String(post.liked));
    likes.textContent = post.like_count || '';
    try {
      await api(`/posts/${post.id}/like`, post.liked ? 'PUT' : 'DELETE');
    } catch {
      post.liked = !post.liked;
      post.like_count += post.liked ? 1 : -1;
      like.setAttribute('aria-pressed', String(post.liked));
      likes.textContent = post.like_count || '';
      toast("That didn't go through.");
    }
  } }, icon('heart'));
  const likes = h('span', { text: post.like_count || '' });
  like.append(likes);
  const replies = h('div', {}, (post.replies ?? []).map(reply => h('p', { class: 'reply' }, h('b', { text: `${reply.author_name ?? 'Someone'} ` }), reply.text)));
  const input = h('input', { type: 'text', placeholder: 'Reply', 'aria-label': 'Reply', maxlength: 500, enterkeyhint: 'send' });
  const form = h('form', { class: 'send', hidden: true, onsubmit: async event => {
    event.preventDefault();
    const text = input.value.trim();
    if (!text) return;
    input.value = '';
    replies.append(h('p', { class: 'reply' }, h('b', { text: 'You ' }), text));
    try { await api(`/posts/${post.id}/replies`, 'POST', { text }); } catch { toast("That reply didn't go through."); refresh(); }
  } }, input);
  const song = { uri: `link:${post.url}`, name: post.title, artists: post.artists, image: post.artwork_url, duration_ms: post.duration_ms };
  const play = () => playShared(post.url);
  return h('article', { class: 'post' },
    h('p', { class: 'who' }, h('b', { text: post.author_name }), ' shared'),
    post.note ? h('p', { class: 'said', text: post.note }) : null,
    h('div', { class: 'row' }, art(song.image), h('button', { class: 'words', onclick: play },
      h('span', { class: `name ${post.source}`, text: post.title }), h('span', { class: 'by', text: post.artists.join(', ') }))),
    replies, form,
    h('div', { class: 'tools' }, like, h('button', { 'aria-label': 'Reply', onclick: () => { form.hidden = false; input.focus(); } }, icon('reply'), h('span', { text: (post.replies ?? []).length || '' }))));
}

function groupScreen() {
  const body = h('div');
  const title = h('h1', { class: 'title', text: 'Group' });
  const chooser = h('div');
  const refresh = () => load(async () => {
    const [account, feed, live, people, lists] = await Promise.all([api('/account'), api('/posts?limit=30'), api('/live').catch(() => null), api('/members').catch(() => null), api('/playlists').catch(() => null)]);
    const groups = account.groups ?? [];
    const chosen = localStorage.getItem(STORE.group);
    const here = groups.find(group => group.id === chosen) ?? groups[0];
    if (here) title.textContent = here.name;
    chooser.replaceChildren(...(groups.length > 1 ? [h('div', { class: 'chips' }, groups.map(group => h('button', { class: 'chip plain', 'aria-pressed': String(group.id === here?.id), text: group.name, onclick: () => {
      localStorage.setItem(STORE.group, group.id);
      refresh();
    } })))] : []));
    const posts = feed.posts ?? [];
    const names = new Map((people?.members ?? []).map(person => [person.user_id, person.display_name]));
    const listening = (live?.presence ?? []).filter(someone => someone.item && someone.playing && someone.user_id !== account.user_id);
    return [
      listening.length ? [h('h2', { class: 'section', text: 'Listening now' }), listening.map(someone => h('div', { class: 'row' }, art(someone.item.artwork_url),
        h('button', { class: 'words', onclick: () => playShared(someone.item.url) },
          h('span', { class: `name ${someone.item.source}`, text: someone.item.title }),
          h('span', { class: 'by', text: `${names.get(someone.user_id) ?? 'Someone'} · ${(someone.item.artists ?? []).join(', ')}` }))))] : null,
      lists?.playlists?.length ? [h('h2', { class: 'section', text: 'Shared playlists' }), h('div', { class: 'cards' }, lists.playlists.slice(0, 6).map(list => card(list.name, `${list.by_name} · ${list.count} songs`, list.artwork_url, () => go({ page: 'shared', id: list.id, name: list.name }))))] : null,
      posts.length ? [listening.length || lists?.playlists?.length ? h('h2', { class: 'section', text: 'Shared songs' }) : null, posts.map(post => postView(post, refresh))] : h('p', { class: 'empty', text: 'Nothing shared yet.' }),
    ];
  }, body);
  refresh();
  return [title, chooser, body];
}

const TABS = [['home', 'Home', homeScreen], ['search', 'Search', searchScreen], ['library', 'Library', libraryScreen], ['group', 'Group', groupScreen]];
/** A playlist someone shared with the group. Each song plays by its own link. */
function sharedScreen() {
  const body = h('div');
  const { id } = current;
  load(async () => {
    const found = await api(`/playlists/${encodeURIComponent(id)}`);
    const items = found.items ?? [];
    return [
      h('h1', { class: 'title', text: found.name }),
      h('p', { class: 'note', text: `${items.length} songs` }),
      items.map(item => h('div', { class: 'row' }, art(item.artwork_url), h('button', { class: 'words', onclick: () => playShared(item.url) },
        h('span', { class: `name ${item.source}`, text: item.title }), h('span', { class: 'by', text: (item.artists ?? []).join(', ') })))),
    ];
  }, body);
  return [backButton(), body];
}

const SCREENS = { list: listScreen, artist: artistScreen, source: sourceScreen, shared: sharedScreen };

/** A tab's page shows as it was left for this long, then reads again. */
const TAB_KEEPS_MS = 3 * 60 * 1000;
const tabViews = new Map();
let shownView = null;
function keepPlace() {
  if (shownView) shownView.scroll = screen.scrollTop;
}

function render(fresh = false) {
  drawn++;
  const tab = TABS.find(([page]) => page === current.page);
  const holder = tab ? tabViews.get(current.page) : current.view;
  let view = !fresh && holder && (!tab || Date.now() - holder.at < TAB_KEEPS_MS) ? holder : null;
  if (!view) {
    const content = (tab ? tab[2] : current.kind === 'source' ? sourceScreen : SCREENS[current.page])();
    view = { node: h('div', { class: 'page' }, h('div', { class: 'banner-slot' }), ...[content].flat(Infinity).filter(Boolean)), scroll: 0, at: Date.now() };
    if (tab) tabViews.set(current.page, view);
    else current.view = view;
  }
  shownView = view;
  screen.replaceChildren(view.node);
  bannerSlot = view.node.querySelector('.banner-slot');
  renderBanner();
  screen.scrollTop = view.scroll;
  markPlaying();
  document.querySelectorAll('nav.tabs button').forEach(button => {
    if (button.dataset.page === (tab ? current.page : tabOf)) button.setAttribute('aria-current', 'page');
    else button.removeAttribute('aria-current');
  });
  if (tab) tabOf = current.page;
}
let tabOf = 'home';

function shell() {
  screen = h('main');
  player.mini = buildMini();
  const tabs = h('nav', { class: 'tabs', 'aria-label': 'Sections' }, TABS.map(([page, label]) => h('button', { 'data-page': page, onclick: () => {
    // Pressed again it goes to the top, and pressed at the top it reads again.
    if (current.page === page) {
      if (screen.scrollTop > 0) screen.scrollTo({ top: 0, behavior: 'smooth' });
      else render(true);
      return;
    }
    trail.length = 0;
    go({ page }, false);
  } }, icon(page), label)));
  app.replaceChildren(screen, player.mini.node, tabs);
  render();
  refreshPlayer();
}

function unlink() {
  for (const key of Object.values(STORE)) localStorage.removeItem(key);
  start();
}

const phoneName = () => (/iPhone/.test(navigator.userAgent) ? 'iPhone' : /Android/.test(navigator.userAgent) ? 'Android phone' : 'Phone');

/** Signing in with the listener's own Spotify app, whose address they add once. */
function spotifyPanel() {
  const client = h('input', { 'aria-label': 'Spotify Client ID', placeholder: 'Client ID', autocapitalize: 'off', autocorrect: 'off', spellcheck: 'false', maxlength: 64, value: localStorage.getItem(STORE.client) ?? '' });
  const problem = h('p', { class: 'problem', role: 'alert', hidden: true });
  const scrim = h('div', { class: 'scrim', onclick: () => { scrim.remove(); panel.remove(); } });
  const address = h('input', { 'aria-label': 'Redirect address', readonly: true, value: redirectUri(), onfocus: event => event.target.select() });
  const panel = h('form', { class: 'door actions sign-in', onsubmit: event => {
    event.preventDefault();
    const id = client.value.trim();
    if (!/^[a-zA-Z0-9]{16,64}$/.test(id)) { problem.textContent = "That doesn't look like a Client ID."; problem.hidden = false; return; }
    spotifySignIn(id);
  } },
    h('p', { text: 'This uses your own Spotify app, the same one as on your computer. In the Spotify developer dashboard, add this redirect address to it once:' }),
    address,
    h('label', { text: 'Your Client ID' }), client, problem,
    h('button', { class: 'primary', type: 'submit', text: 'Continue to Spotify' }));
  document.body.append(scrim, panel);
}

// ---------------------------------------------------------------- linking
function linkScreen() {
  const problem = h('p', { class: 'problem', role: 'alert', hidden: true });
  const input = h('input', { class: 'code', 'aria-label': 'Code from your computer', maxlength: 9, autocapitalize: 'characters', autocorrect: 'off', autocomplete: 'one-time-code', spellcheck: 'false', enterkeyhint: 'go', required: true });
  const button = h('button', { class: 'primary', type: 'submit', text: 'Link this phone' });
  const form = h('form', { class: 'door', onsubmit: async event => {
    event.preventDefault();
    button.disabled = true;
    problem.hidden = true;
    try {
      const linked = await api('/link/claim', 'POST', { code: input.value, device: phoneName() });
      localStorage.setItem(STORE.secret, linked.secret);
      localStorage.setItem(STORE.name, linked.display_name);
      if (linked.client_id) localStorage.setItem(STORE.client, linked.client_id);
      start();
    } catch (error) {
      problem.textContent = error.message;
      problem.hidden = false;
      button.disabled = false;
    }
  } },
    h('img', { class: 'logo', src: '/icon.svg', alt: '', width: 64, height: 64 }),
    h('h1', { text: 'Encore' }),
    h('p', { text: 'Use your own account on this phone: your library, your groups, and playback on your computer.' }),
    h('p', { class: 'small', text: 'On your computer, open Encore, go to Group, then People, and press Link a phone. Type the code here.' }),
    input, problem, button,
    h('button', { class: 'quiet', type: 'button', text: 'Sign in with Spotify instead', onclick: spotifyPanel }));
  app.replaceChildren(h('div', { class: 'door-page' }, form));
  input.focus();
}

let polling = false;
let arrival = null;
async function arrive() {
  if (location.pathname === '/callback') {
    try { await spotifyReturn(); } catch (error) { arrival = error.message; }
  }
  start();
  if (arrival) { toast(arrival); arrival = null; }
}
function start() {
  if (localStorage.getItem(STORE.secret)) {
    shell();
    if (!polling) { polling = true; pollState(); }
  } else {
    player.mini = null;
    linkScreen();
  }
}
arrive();

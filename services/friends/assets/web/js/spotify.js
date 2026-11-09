// Spotify itself: the listener's own app signed in in this browser.

import { STORE, keepSecret, phoneName } from './auth.js';
import { api } from './service.js';
import { player } from './state.js';

const SPOTIFY_SCOPES = 'user-read-private user-read-email playlist-read-private playlist-read-collaborative user-library-read user-library-modify user-read-playback-state user-modify-playback-state user-read-currently-playing user-top-read';
export const redirectUri = () => `${location.origin}/callback`;
const base64url = bytes => btoa(String.fromCharCode(...new Uint8Array(bytes))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
export const hasSpotify = () => !!localStorage.getItem(STORE.spotify);

export async function spotifySignIn(clientId) {
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
export async function spotifyReturn() {
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
  keepSecret(joined.secret);
  localStorage.setItem(STORE.name, joined.display_name);
}

async function spotifyAccess() {
  let saved = JSON.parse(localStorage.getItem(STORE.spotify) ?? 'null');
  if (!saved) throw new Error('Connect Spotify first.');
  if (Date.now() > saved.until - 60000) saved = await spotifyTokens({ grant_type: 'refresh_token', refresh_token: saved.refresh });
  return saved.access;
}

export async function web(path, method = 'GET', body) {
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

export const spotifySong = track => ({
  uri: track.uri,
  name: track.name,
  artists: (track.artists ?? []).map(artist => artist.name),
  artist_id: track.artists?.[0]?.id,
  image: track.album?.images?.[1]?.url ?? track.album?.images?.[0]?.url,
  duration_ms: track.duration_ms,
});
const playable = item => item && item.type === 'track' && item.uri?.startsWith('spotify:track:');

/** The same questions the computer answers, for Spotify alone. */
export const DIRECT = {
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
export const DIRECT_ACTS = {
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
    // Nothing awake to send it to: this page never opens another app to play.
    if (!device) throw new Error('No Spotify device is awake. Open Spotify first.');
    await web(`/me/player/play?device_id=${encodeURIComponent(device.id)}`, 'PUT', { uris: songs, offset: { uri: wanted } });
  },
};

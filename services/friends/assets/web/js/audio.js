// The lock screen, headphones, and the picture they show.

import { safeImage } from './dom.js';
import { here, hooks } from './state.js';

/** The lock screen and headphones show the song and work the buttons. */
export function mediaSession(song) {
  if (!('mediaSession' in navigator)) return;
  const image = safeImage(song.image);
  navigator.mediaSession.metadata = new MediaMetadata({
    title: song.name, artist: song.artists.join(', '), artwork: artworkFor(image),
  });
  lockScreenHandlers();
  lockScreenState();
}

/** Next and previous, not ±10s: iOS only shows those without the seek buttons. */
export function lockScreenHandlers() {
  if (!('mediaSession' in navigator) || !here.audio) return;
  const handlers = {
    play: () => hooks.hereActs.toggle(), pause: () => hooks.hereActs.toggle(),
    nexttrack: () => hooks.hereActs.next(), previoustrack: () => hooks.hereActs.previous(),
    seekto: details => hooks.hereActs.seek({ ms: (details.seekTime ?? 0) * 1000 }),
    seekbackward: null, seekforward: null,
  };
  for (const [action, handler] of Object.entries(handlers)) {
    try { navigator.mediaSession.setActionHandler(action, handler); } catch { /* not every phone has every one */ }
  }
}

/** What the lock screen shows: playing or paused, and how far along. */
export function lockScreenState() {
  if (!('mediaSession' in navigator)) return;
  const audio = here.audio;
  const mine = here.active && here.playingUri === here.list[here.index]?.uri;
  navigator.mediaSession.playbackState = !here.active || !audio ? 'none' : mine && !audio.paused ? 'playing' : 'paused';
  if (!mine || !audio || !Number.isFinite(audio.duration) || audio.duration <= 0) return;
  try {
    navigator.mediaSession.setPositionState({
      duration: audio.duration, playbackRate: audio.playbackRate || 1,
      position: Math.min(Math.max(audio.currentTime, 0), audio.duration),
    });
  } catch { /* an older phone */ }
}

/** The size an image address names, when it names one. */
function imageSize(url) {
  const named = /-t?(\d{2,4})x(\d{2,4})\./.exec(url);
  if (named) return `${named[1]}x${named[2]}`;
  // Spotify's covers end in a code for their size.
  return { '0000b273': '640x640', '00001e02': '300x300', '00004851': '64x64' }[/ab67616d([0-9a-f]{8})/.exec(url)?.[1]] ?? null;
}

/** The lock screen's picture, at each size the phone asks for, or its true one. */
export function artworkFor(image) {
  if (!image) return [];
  const known = imageSize(image);
  return (known ? [known] : ['96x96', '192x192', '256x256', '512x512']).map(sizes => ({ src: image, sizes }));
}

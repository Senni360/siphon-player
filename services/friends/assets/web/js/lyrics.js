// The words of a song, read by this phone itself.
//
// The computer only knows the words of what it plays. A song playing on this
// phone, or any song while the computer is off, is looked up here instead, at
// LRCLIB: the same public library the computer reads. Only the song's name and
// its artist are sent. A song nobody has written down is no error.

import { plain } from './words.js';

const API = 'https://lrclib.net/api';
const TIMEOUT_MS = 8000;
/** How far a transcript's length may be from the song's and still be this song. */
const LENGTH_SLACK_S = 3;
const KEEP = 30;

/** A title without what a service adds after it: "Song - 2011 Remaster", "Song (feat. X)". */
export function baseTitle(title) {
  const cut = String(title ?? '').replace(/\s+-\s+.*$/, '').replace(/\s*[([][^)\]]*[)\]]\s*/g, ' ').trim();
  return cut || String(title ?? '');
}

/** "[01:02.50] words" lines, in order. A line with several times is sung several times. */
export function parseSynced(text) {
  const lines = [];
  for (const raw of String(text ?? '').split(/\r?\n/)) {
    const stamps = [...raw.matchAll(/\[(\d{1,3}):(\d{1,2})(?:[.:](\d{1,3}))?\]/g)];
    if (!stamps.length) continue;
    const words = raw.replace(/\[[^\]]*\]/g, '').trim();
    for (const [, minutes, seconds, part = '0'] of stamps) {
      lines.push({ at_ms: Number(minutes) * 60000 + Number(seconds) * 1000 + Math.round(Number(`0.${part}`) * 1000), text: words });
    }
  }
  return lines.sort((first, second) => first.at_ms - second.at_ms);
}

/** One transcript as the player shows it, or null when it holds no words. */
export function wordsOf(record) {
  if (!record || typeof record !== 'object') return null;
  if (record.instrumental) return { lines: [], synced: false, instrumental: true };
  const synced = parseSynced(record.syncedLyrics);
  if (synced.length) return { lines: synced, synced: true, instrumental: false };
  const lines = String(record.plainLyrics ?? '').split(/\r?\n/).map(text => ({ text: text.trim() }));
  while (lines.length && !lines[lines.length - 1].text) lines.pop();
  return lines.some(line => line.text) ? { lines, synced: false, instrumental: false } : null;
}

/** Whether a transcript is of this very song: its name, its artist, and near enough its length. */
export function sameSong(record, song) {
  const title = plain(record?.trackName ?? record?.name);
  if (!title || (title !== plain(song.name) && title !== plain(baseTitle(song.name)))) return false;
  const theirs = plain(record.artistName);
  const artists = (song.artists ?? []).map(plain).filter(Boolean);
  if (!theirs || !artists.some(artist => theirs === artist || theirs.includes(artist) || artist.includes(theirs))) return false;
  const length = (song.duration_ms ?? 0) / 1000;
  return !(length > 0 && Number.isFinite(record.duration) && record.duration > 0) || Math.abs(record.duration - length) <= LENGTH_SLACK_S;
}

/** The best of several transcripts of this song: timed words before plain ones. */
export function pick(records, song) {
  let best = null;
  for (const record of Array.isArray(records) ? records : []) {
    if (!sameSong(record, song)) continue;
    const words = wordsOf(record);
    if (!words) continue;
    if (!best || (words.synced && !best.synced)) best = words;
    if (best.synced) break;
  }
  return best;
}

async function read(path, params) {
  let answer;
  try {
    answer = await fetch(`${API}${path}?${new URLSearchParams(params)}`, { signal: AbortSignal.timeout(TIMEOUT_MS), credentials: 'omit', referrerPolicy: 'no-referrer' });
  } catch { throw new Error("Couldn't reach the lyrics. Check the connection and open them again."); }
  // 404 is nobody having written this song down, and 400 a song it can't be asked about.
  if (answer.status === 404 || answer.status === 400) return null;
  if (!answer.ok) throw new Error('The lyrics are busy right now. Open them again in a moment.');
  return await answer.json().catch(() => null);
}

const found = new Map();
/**
 * The words of `song`: `{ lines, synced, instrumental }`, or null when nobody has
 * written them down. Throws only when the library could not be reached.
 */
export function findLyrics(song) {
  const artist = (song.artists ?? [])[0];
  if (!song?.name || !artist) return Promise.resolve(null);
  const key = `${song.uri}|${song.name}|${artist}`;
  const held = found.get(key);
  if (held) return held;
  const looking = (async () => {
    // Asked as a search, which answers an empty list where a single look would
    // answer "not found": no words is an ordinary answer, never a failed request.
    for (const title of [...new Set([song.name, baseTitle(song.name)])]) {
      const words = pick(await read('/search', { track_name: title, artist_name: artist }), song);
      if (words) return words;
    }
    return null;
  })();
  found.set(key, looking);
  // A failed look is asked again next time; an answer, even "none", is kept a while.
  looking.catch(() => found.delete(key));
  if (found.size > KEEP) found.delete(found.keys().next().value);
  return looking;
}

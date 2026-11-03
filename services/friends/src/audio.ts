import { randomUUID } from 'node:crypto';
import { body, fail, json, text } from './http.js';
import type { SharedAudio, State, Storage, User } from './storage.js';

/**
 * Songs people share from their own files. Vercel Blob's free plan holds 1 GB
 * and sends 10 GB a month, and going over either blocks it for 30 days, so these
 * limits sit below both. Nobody has an upload allowance: when the store is full,
 * the oldest songs make room.
 */
export const MAX_SONG_MS = 15 * 60 * 1000;
export const MAX_SONG_BYTES = 40 * 1024 * 1024;
export const STORE_BYTES = 900 * 1024 * 1024;
export const MAX_SONGS = 1000;
export const MONTH_BYTES = 9 * 1024 * 1024 * 1024;
export const UPLOADS_PER_HOUR = 40;
/** An upload that never finished stops holding room after this long. */
export const RESERVED_MS = 20 * 60 * 1000;
export const TOKEN_MS = 15 * 60 * 1000;

const TYPES: Record<string, string> = { mp3: 'audio/mpeg', flac: 'audio/flac', m4a: 'audio/mp4', aac: 'audio/aac' };

/** The file store, separate so tests never reach the network. */
export interface AudioStore {
  /** Lets one client upload one file of at most `bytes` to `pathname`. */
  uploadToken(pathname: string, bytes: number, contentType: string, until: number): Promise<string>;
  /** The stored file, or null if it never arrived. */
  find(pathname: string): Promise<{ url: string; size: number } | null>;
  remove(pathnames: string[]): Promise<void>;
}

export function configuredAudioStore(env: NodeJS.ProcessEnv): AudioStore | null {
  const token = env.BLOB_READ_WRITE_TOKEN;
  if (!token) return null;
  return {
    async uploadToken(pathname, bytes, contentType, until) {
      const { generateClientTokenFromReadWriteToken } = await import('@vercel/blob/client');
      return generateClientTokenFromReadWriteToken({
        token, pathname, maximumSizeInBytes: bytes, allowedContentTypes: [contentType],
        validUntil: until, addRandomSuffix: false, allowOverwrite: false,
      });
    },
    async find(pathname) {
      const { head, BlobNotFoundError } = await import('@vercel/blob');
      try {
        const found = await head(pathname, { token });
        return { url: found.url, size: found.size };
      } catch (error) {
        if (error instanceof BlobNotFoundError) return null;
        throw error;
      }
    },
    async remove(pathnames) {
      if (!pathnames.length) return;
      const { del } = await import('@vercel/blob');
      await del(pathnames, { token });
    },
  };
}

const pathnameOf = (song: Pick<SharedAudio, 'id' | 'ext'>) => `audio/${song.id}.${song.ext}`;

/** The month's name, which starts the download count afresh when it changes. */
export const monthOf = (time: number) => new Date(time).toISOString().slice(0, 7);

/**
 * Takes out what no longer fits: unfinished uploads past their time, then the
 * oldest songs until `incoming` more bytes fit. Returns the files to delete.
 * A song being added is never its own victim, because it arrives through `incoming`.
 */
export function makeRoom(state: State, incoming: number, now: number): SharedAudio[] {
  const songs = state.audio ??= [];
  const gone: SharedAudio[] = [];
  state.audio = songs.filter(song => {
    const stale = song.url === null && now - song.created_at > RESERVED_MS;
    if (stale) gone.push(song);
    return !stale;
  });
  const oldestFirst = [...state.audio].filter(song => song.url !== null).sort((a, b) => a.created_at - b.created_at);
  let used = state.audio.reduce((sum, song) => sum + song.size, 0);
  let count = state.audio.length;
  for (const song of oldestFirst) {
    if (used + incoming <= STORE_BYTES && count < MAX_SONGS) break;
    used -= song.size;
    count -= 1;
    gone.push(song);
  }
  const removed = new Set(gone.map(song => song.id));
  state.audio = state.audio.filter(song => !removed.has(song.id));
  return gone;
}

export interface AudioContext {
  request: Request;
  path: string;
  user: User;
  groupId: string;
  mayShare: boolean;
  /** Whether this person may remove what others shared. */
  manages: boolean;
  state: State;
  storage: Storage;
  store: AudioStore | null;
  clock: () => number;
  signal: AbortSignal;
  change: <T>(update: (state: State, user: User) => T) => Promise<T>;
}

/** The route's answer, or null when the path is not one of these. */
export async function audioRoute(context: AudioContext): Promise<Response | null> {
  const { request, path, user, groupId, state, storage, clock, signal, change } = context;
  const method = request.method;
  const one = /^\/audio\/([a-f0-9-]{36})(?:\/(complete|play))?$/.exec(path);
  if (path !== '/audio' && !one) return null;
  const store = context.store ?? fail(503, 'Sharing songs is not set up on this service');

  if (path === '/audio' && method === 'GET') {
    const names = new Map(state.users.map(u => [u.user_id, u.display_name]));
    const songs = (state.audio ?? []).filter(song => song.group_id === groupId && song.url !== null)
      .sort((a, b) => b.created_at - a.created_at)
      .map(song => ({
        id: song.id, title: song.title, artists: song.artists, duration_ms: song.duration_ms, size: song.size,
        by: song.by, by_name: names.get(song.by) ?? 'Former member', created_at: song.created_at,
      }));
    const used = (state.audio ?? []).reduce((sum, song) => sum + song.size, 0);
    return json({ songs, used, capacity: STORE_BYTES });
  }

  if (path === '/audio' && method === 'POST') {
    if (!context.mayShare) return fail(403, 'An admin turned off sharing for you in this group');
    const input = await body(request, signal);
    const title = text(input.title, 'title', 300);
    if (!Array.isArray(input.artists) || input.artists.length > 20) return fail(400, 'Invalid artists');
    const artists = (input.artists as unknown[]).map(artist => text(artist, 'artist', 200));
    const duration = Number(input.duration_ms);
    if (!Number.isSafeInteger(duration) || duration <= 0) return fail(400, 'Invalid duration');
    if (duration > MAX_SONG_MS) return fail(413, 'That song is too long to share');
    const size = Number(input.size);
    if (!Number.isSafeInteger(size) || size <= 0) return fail(400, 'Invalid size');
    if (size > MAX_SONG_BYTES) return fail(413, 'That file is too big to share');
    const ext = typeof input.ext === 'string' ? input.ext.toLowerCase() : '';
    const contentType = Object.hasOwn(TYPES, ext) ? TYPES[ext] : fail(415, 'That file type cannot be shared');
    if (!await storage.limit(`user:${user.user_id}:audio-upload`, UPLOADS_PER_HOUR, 3600, signal)) return fail(429, 'Too many uploads, try again later', 3600);
    const song: SharedAudio = { id: randomUUID(), group_id: groupId, by: user.user_id, title, artists, duration_ms: duration, size, ext, created_at: clock(), url: null };
    const gone = await change(current => {
      const removed = makeRoom(current, size, clock());
      current.audio!.push(song);
      return removed;
    });
    await store.remove(gone.map(pathnameOf));
    const pathname = pathnameOf(song);
    const token = await store.uploadToken(pathname, size, contentType, clock() + TOKEN_MS);
    return json({ id: song.id, pathname, token, content_type: contentType }, 201);
  }

  if (!one) return fail(404, 'Route not found');
  const id = one[1];
  const song = (state.audio ?? []).find(row => row.id === id && row.group_id === groupId) ?? fail(404, 'That song is gone');

  if (one[2] === 'complete' && method === 'POST') {
    if (song.by !== user.user_id) return fail(403, 'Only who uploaded a song can finish it');
    const found = await store.find(pathnameOf(song)) ?? fail(409, 'The upload did not arrive');
    const url = found.url;
    await change(current => {
      const row = (current.audio ?? []).find(other => other.id === id) ?? fail(404, 'That song is gone');
      row.url = url;
      row.size = found.size;
    });
    return json({ id, title: song.title, artists: song.artists, duration_ms: song.duration_ms, size: found.size, by: song.by, by_name: user.display_name, created_at: song.created_at });
  }

  if (one[2] === 'play' && method === 'POST') {
    if (song.url === null) return fail(404, 'That song is gone');
    const month = monthOf(clock());
    const spent = state.audio_month?.month === month ? state.audio_month.bytes : 0;
    if (spent + song.size > MONTH_BYTES) return fail(429, 'Shared songs used up this month\'s downloads');
    await change(current => {
      const count = current.audio_month?.month === month ? current.audio_month : (current.audio_month = { month, bytes: 0 });
      count.bytes += song.size;
    });
    return json({ url: song.url, content_type: TYPES[song.ext], duration_ms: song.duration_ms });
  }

  if (!one[2] && method === 'DELETE') {
    if (song.by !== user.user_id && !context.manages) return fail(403, 'You can only delete your own songs');
    await change(current => { current.audio = (current.audio ?? []).filter(row => row.id !== id); });
    await store.remove([pathnameOf(song)]);
    return json({ ok: true });
  }
  return fail(404, 'Route not found');
}

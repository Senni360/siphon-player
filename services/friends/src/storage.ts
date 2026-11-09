import { httpsUrl, sourceUrl } from './urls.js';

/** One signed-in computer. The secret it holds is only ever stored hashed. */
export interface Device {
  id: string;
  hash: string;
  name: string;
  created_at: number;
  /** How the device joined. A linked phone can never act as a computer. */
  kind?: 'phone';
  /** When this device was last used. When the account holds too many, the
   *  least recently seen is the one that goes, so a used device survives. */
  seen?: number;
}
export interface User {
  user_id: string;
  display_name: string;
  /** The first secret of a member from before accounts; unusable random bytes after. */
  secret_hash: string;
  /** Who this account is, as Spotify said when the app proved it. */
  spotify_id?: string;
  /** A label for friends and new devices. SoundCloud's own grant never leaves the app. */
  soundcloud?: { id: string; username: string };
  devices?: Device[];
}
export interface Rights { share: boolean; host: boolean; queue: boolean; invite: boolean }
export interface Member { user_id: string; role: 'owner' | 'admin' | 'member'; rights: Rights }
export interface Group {
  id: string;
  name: string;
  owner_id: string | null;
  created_at: number;
  members: Member[];
  invite: string | null;
  /** Spotify ids an admin added who have not signed in yet. */
  pending: string[];
  /** Whom an admin removed: an old invite does not bring them back. */
  removed: string[];
}
/** A playlist someone shared. Its songs sit in their own key, to keep this document small. */
export interface SharedPlaylist { id: string; group_id: string; name: string; by: string; created_at: number; count: number; artwork_url: string | null; to?: string[] }
export interface Payload {
  url: string;
  source: 'spotify' | 'soundcloud' | 'youtube';
  title: string;
  artists: string[];
  artwork_url: string | null;
  duration_ms: number;
  note?: string;
  /** Member ids a share was sent to. Absent or empty: the whole group. */
  to?: string[];
}
export interface Reply { id: string; author_id: string; created_at: number; text: string }
export interface Post extends Payload {
  id: string;
  /** Absent: the group from before groups. */
  group_id?: string;
  author_id: string;
  created_at: number;
  likes: string[];
  replies?: Reply[];
  /** Jev's estimate, by member id, that the member will enjoy this song. */
  affinity?: Record<string, number>;
}
/** A song someone shared from their own files. `url` stays null until the upload finishes. */
export interface SharedAudio { id: string; group_id: string; by: string; title: string; artists: string[]; duration_ms: number; size: number; ext: string; created_at: number; url: string | null }
export interface State { revision: number; users: User[]; posts: Post[]; groups?: Group[]; playlists?: SharedPlaylist[]; audio?: SharedAudio[]; audio_month?: { month: string; bytes: number } }
export const empty = (): State => ({ revision: 0, users: [], posts: [] });
export const MAX_USERS = 500;
export const MAX_POSTS = 5000;
export const MAX_REPLIES = 200;
export const MAX_RECIPIENTS = 20;
export const MAX_STATE_BYTES = 8 * 1024 * 1024;
// A REST provider may encode a stored character as six ASCII bytes (\uXXXX).
const MAX_RESPONSE_BYTES = MAX_STATE_BYTES * 6 + 1024;
export class StorageFullError extends Error {}

function encodedState(state: State): string {
  if (!Number.isSafeInteger(state.revision) || state.revision < 0 || state.revision >= Number.MAX_SAFE_INTEGER) throw new Error('Invalid state revision');
  const encoded = JSON.stringify(state);
  if (Buffer.byteLength(encoded) > MAX_STATE_BYTES) throw new StorageFullError('Group storage is full');
  return encoded;
}

function storedState(value: unknown): State {
  if (!value || typeof value !== 'object') throw new Error('Invalid stored group');
  const state = value as State;
  const id = (value: unknown): value is string => typeof value === 'string' && /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(value);
  const text = (value: unknown, max: number, empty = false): value is string => typeof value === 'string' && value.length <= max && (empty || !!value.trim()) && !/[\x00-\x1f\x7f]/.test(value);
  if (!Number.isSafeInteger(state.revision) || state.revision < 0 || state.revision >= Number.MAX_SAFE_INTEGER ||
      !Array.isArray(state.users) || state.users.length > MAX_USERS || !Array.isArray(state.posts) || state.posts.length > MAX_POSTS ||
      !state.users.every(u => u && id(u.user_id) && text(u.display_name, 40) && typeof u.secret_hash === 'string' && /^[a-f0-9]{64}$/.test(u.secret_hash)) ||
      new Set(state.users.map(u => u.user_id)).size !== state.users.length ||
      new Set(state.users.map(u => u.secret_hash)).size !== state.users.length ||
      !state.users.every(u => (u.spotify_id === undefined || text(u.spotify_id, 128)) &&
        (u.soundcloud === undefined || !!u.soundcloud && text(u.soundcloud.id, 40) && text(u.soundcloud.username, 100)) &&
        (u.devices === undefined || Array.isArray(u.devices) && u.devices.length <= 20 && u.devices.every(d => d && id(d.id) && /^[a-f0-9]{64}$/.test(d.hash) && text(d.name, 60) && Number.isSafeInteger(d.created_at) && (d.seen === undefined || Number.isSafeInteger(d.seen) && d.seen >= 0)))) ||
      new Set(state.users.flatMap(u => u.spotify_id ? [u.spotify_id] : [])).size !== state.users.filter(u => u.spotify_id).length ||
      (state.groups !== undefined && !(Array.isArray(state.groups) && state.groups.every(g => g && id(g.id) && text(g.name, 60) && Array.isArray(g.members) && g.members.every(m => m && id(m.user_id) && ['owner', 'admin', 'member'].includes(m.role) && m.rights && typeof m.rights === 'object') && Array.isArray(g.pending) && Array.isArray(g.removed)))) ||
      (state.playlists !== undefined && !(Array.isArray(state.playlists) && state.playlists.every(p => p && id(p.id) && id(p.group_id) && id(p.by) && text(p.name, 100) && Number.isSafeInteger(p.count))))) {
    throw new Error('Invalid stored group');
  }
  const members = new Set(state.users.map(u => u.user_id));
  if (!state.posts.every(p => p && id(p.id) && id(p.author_id) &&
      Number.isSafeInteger(p.created_at) && p.created_at >= 0 &&
      sourceUrl(p.url, p.source) !== null && text(p.title, 300) &&
      Array.isArray(p.artists) && p.artists.length <= 20 && p.artists.every(a => text(a, 200)) &&
      (p.artwork_url === null || httpsUrl(p.artwork_url) !== null) &&
      Number.isSafeInteger(p.duration_ms) && p.duration_ms >= 0 && p.duration_ms <= 604800000 &&
      (p.note === undefined || text(p.note, 280, true)) && Array.isArray(p.likes) &&
      p.likes.length <= MAX_USERS && new Set(p.likes).size === p.likes.length && p.likes.every(id => members.has(id)) &&
      (p.group_id === undefined || id(p.group_id)) && (p.to === undefined || Array.isArray(p.to) && p.to.length <= MAX_RECIPIENTS && new Set(p.to).size === p.to.length && p.to.every(id)) &&
      (p.replies === undefined || Array.isArray(p.replies) && p.replies.length <= MAX_REPLIES &&
        p.replies.every(r => r && id(r.id) && id(r.author_id) && Number.isSafeInteger(r.created_at) && r.created_at >= 0 && text(r.text, 500)) &&
        new Set(p.replies.map(r => r.id)).size === p.replies.length) &&
      (p.affinity === undefined || !!p.affinity && typeof p.affinity === 'object' && !Array.isArray(p.affinity) &&
        Object.keys(p.affinity).length <= MAX_USERS && Object.entries(p.affinity).every(([key, value]) => id(key) && typeof value === 'number' && value >= 0 && value <= 1))) ||
      new Set(state.posts.map(p => p.id)).size !== state.posts.length) throw new Error('Invalid stored group');
  return state;
}

// Optimistic transactions keep joins, deletes and hearts atomic across Vercel
// instances. The group is deliberately small; one document keeps the adapter
// dependency-free. No read/modify/write is allowed without compareAndSet.
export interface Storage {
  read(signal?: AbortSignal): Promise<State>;
  compareAndSet(previous: number, next: State, signal?: AbortSignal): Promise<boolean>;
  limit(key: string, maximum: number, seconds: number, signal?: AbortSignal): Promise<boolean>;
  // Live keys sit beside the document and expire by themselves.
  liveRead(keys: string[], signal?: AbortSignal): Promise<(string | null)[]>;
  livePut(key: string, value: string, seconds: number, signal?: AbortSignal): Promise<void>;
  liveDelete(key: string, signal?: AbortSignal): Promise<void>;
  /** Stores `value` if the key's JSON `revision` is still `previous` (0 when absent). */
  liveCas(key: string, previous: number, value: string, seconds: number, signal?: AbortSignal): Promise<boolean>;
  /** Tells whoever is listening on `channel` right now. Nobody listening is fine. */
  livePublish(channel: string, message: string, signal?: AbortSignal): Promise<void>;
  /** What's said on `channel` from now until `signal` ends, one message at a time. */
  liveListen(channel: string, signal: AbortSignal): AsyncIterable<string>;
}

/** The longest message a channel carries; anything longer is dropped. */
export const MAX_MESSAGE_BYTES = 32 * 1024;

/** Messages handed to a listener as they arrive, until its signal ends. */
async function* queued(subscribe: (deliver: (message: string) => void) => () => void, signal: AbortSignal): AsyncGenerator<string> {
  const waiting: string[] = [];
  let wake: (() => void) | null = null;
  const unsubscribe = subscribe(message => { waiting.push(message); wake?.(); });
  const ended = () => wake?.();
  signal.addEventListener('abort', ended, { once: true });
  try {
    while (!signal.aborted) {
      const message = waiting.shift();
      if (message !== undefined) { yield message; continue; }
      await new Promise<void>(resolve => { wake = resolve; });
      wake = null;
    }
  } finally {
    unsubscribe();
    signal.removeEventListener('abort', ended);
  }
}

export class MemoryStorage implements Storage {
  private state = empty();
  private limits = new Map<string, { count: number; until: number }>();
  private live = new Map<string, { value: string; until: number }>();
  private listeners = new Map<string, Set<(message: string) => void>>();
  constructor(private clock: () => number = Date.now) {}
  async livePublish(channel: string, message: string, signal?: AbortSignal) {
    signal?.throwIfAborted();
    for (const deliver of this.listeners.get(channel) ?? []) deliver(message);
  }
  liveListen(channel: string, signal: AbortSignal): AsyncIterable<string> {
    return queued(deliver => {
      const set = this.listeners.get(channel) ?? new Set();
      set.add(deliver);
      this.listeners.set(channel, set);
      return () => { set.delete(deliver); if (!set.size) this.listeners.delete(channel); };
    }, signal);
  }
  private liveValue(key: string) {
    const entry = this.live.get(key);
    if (entry && entry.until <= this.clock()) this.live.delete(key);
    return this.live.get(key)?.value ?? null;
  }
  async liveRead(keys: string[], signal?: AbortSignal) { signal?.throwIfAborted(); return keys.map(key => this.liveValue(key)); }
  async livePut(key: string, value: string, seconds: number, signal?: AbortSignal) {
    signal?.throwIfAborted();
    this.live.set(key, { value, until: this.clock() + seconds * 1000 });
  }
  async liveDelete(key: string, signal?: AbortSignal) { signal?.throwIfAborted(); this.live.delete(key); }
  async liveCas(key: string, previous: number, value: string, seconds: number, signal?: AbortSignal) {
    signal?.throwIfAborted();
    const old = this.liveValue(key);
    if ((old === null ? 0 : (JSON.parse(old) as { revision: number }).revision) !== previous) return false;
    await this.livePut(key, value, seconds);
    return true;
  }
  async read(signal?: AbortSignal) { signal?.throwIfAborted(); return structuredClone(this.state); }
  async compareAndSet(previous: number, next: State, signal?: AbortSignal) {
    signal?.throwIfAborted();
    if (this.state.revision !== previous) return false;
    encodedState(next);
    this.state = structuredClone(next);
    return true;
  }
  async limit(key: string, maximum: number, seconds: number, signal?: AbortSignal) {
    signal?.throwIfAborted();
    const now = Date.now();
    for (const [key, value] of this.limits) if (value.until <= now) this.limits.delete(key);
    const bucket = this.limits.get(key) ?? { count: 0, until: now + seconds * 1000 };
    this.limits.set(key, bucket);
    return ++bucket.count <= maximum;
  }
}

const CAS = `local old = redis.call('GET', KEYS[1])
local revision = 0
if old then revision = cjson.decode(old).revision end
if revision ~= tonumber(ARGV[1]) then return 0 end
redis.call('SET', KEYS[1], ARGV[2]); return 1`;
const LIVE_CAS = `local old = redis.call('GET', KEYS[1])
local revision = 0
if old then revision = cjson.decode(old).revision end
if revision ~= tonumber(ARGV[1]) then return 0 end
redis.call('SET', KEYS[1], ARGV[2], 'EX', ARGV[3]); return 1`;
const LIMIT = `local n = redis.call('INCR', KEYS[1])
if n == 1 then redis.call('EXPIRE', KEYS[1], ARGV[1]) end
return n`;

export class RedisStorage implements Storage {
  constructor(private url: string, private token: string, private prefix = 'siphon:friends:v1') {
    const endpoint = new URL(url);
    if (endpoint.protocol !== 'https:' || endpoint.username || endpoint.password || endpoint.search || endpoint.hash || !token) {
      throw new Error('Invalid Redis REST configuration');
    }
  }
  private async command(args: (string | number)[], caller?: AbortSignal): Promise<unknown> {
    const signal = AbortSignal.any([AbortSignal.timeout(5000), ...(caller ? [caller] : [])]);
    signal.throwIfAborted();
    const response = await fetch(this.url, {
      method: 'POST', headers: { Authorization: `Bearer ${this.token}`, 'Content-Type': 'application/json' },
      body: JSON.stringify(args), signal, redirect: 'error',
    });
    if (!response.ok || Number(response.headers.get('content-length')) > MAX_RESPONSE_BYTES) {
      await response.body?.cancel();
      throw new Error('Storage unavailable');
    }
    const reader = response.body?.getReader();
    if (!reader) throw new Error('Storage unavailable');
    const chunks: Uint8Array[] = [];
    let size = 0;
    const cancel = () => { void reader.cancel().catch(() => {}); };
    signal.addEventListener('abort', cancel, { once: true });
    try {
      signal.throwIfAborted();
      while (true) {
        const part = await reader.read();
        signal.throwIfAborted();
        if (part.done) break;
        size += part.value.length;
        if (size > MAX_RESPONSE_BYTES) throw new Error('Storage unavailable');
        chunks.push(part.value);
      }
    } catch (error) {
      await reader.cancel().catch(() => {});
      throw error;
    } finally { signal.removeEventListener('abort', cancel); reader.releaseLock(); }
    const value = JSON.parse(Buffer.concat(chunks, size).toString('utf8')) as { result?: unknown; error?: unknown } | null;
    if (!value || typeof value !== 'object' || 'error' in value || !('result' in value)) throw new Error('Storage unavailable');
    return value.result;
  }
  async read(signal?: AbortSignal): Promise<State> {
    const result = await this.command(['GET', `${this.prefix}:state`], signal);
    if (result === null) return empty();
    if (typeof result !== 'string' || Buffer.byteLength(result) > MAX_STATE_BYTES) throw new Error('Invalid stored group');
    return storedState(JSON.parse(result));
  }
  async compareAndSet(previous: number, next: State, signal?: AbortSignal) {
    signal?.throwIfAborted();
    const result = await this.command(['EVAL', CAS, 1, `${this.prefix}:state`, previous, encodedState(next)], signal);
    if (result !== 0 && result !== 1) throw new Error('Storage unavailable');
    return result === 1;
  }
  private liveKey(key: string) { return `${this.prefix}:live:${key}`; }
  async liveRead(keys: string[], signal?: AbortSignal) {
    const result = await this.command(['MGET', ...keys.map(key => this.liveKey(key))], signal);
    if (!Array.isArray(result) || result.length !== keys.length) throw new Error('Storage unavailable');
    return result.map(value => typeof value === 'string' ? value : null);
  }
  async livePut(key: string, value: string, seconds: number, signal?: AbortSignal) {
    await this.command(['SET', this.liveKey(key), value, 'EX', seconds], signal);
  }
  async liveDelete(key: string, signal?: AbortSignal) { await this.command(['DEL', this.liveKey(key)], signal); }
  async liveCas(key: string, previous: number, value: string, seconds: number, signal?: AbortSignal) {
    const result = await this.command(['EVAL', LIVE_CAS, 1, this.liveKey(key), previous, value, seconds], signal);
    if (result !== 0 && result !== 1) throw new Error('Storage unavailable');
    return result === 1;
  }
  async livePublish(channel: string, message: string, signal?: AbortSignal) {
    await this.command(['PUBLISH', this.liveKey(channel), message], signal);
  }
  /** Upstash's REST subscribe: a stream of `data: message,<channel>,<payload>` lines. */
  async *liveListen(channel: string, signal: AbortSignal): AsyncGenerator<string> {
    const name = this.liveKey(channel);
    const endpoint = new URL(this.url);
    endpoint.pathname = `${endpoint.pathname.replace(/\/$/, '')}/subscribe/${encodeURIComponent(name)}`;
    const response = await fetch(endpoint, {
      method: 'POST', headers: { Authorization: `Bearer ${this.token}`, Accept: 'text/event-stream' },
      signal, redirect: 'error',
    });
    if (!response.ok || !response.body) {
      await response.body?.cancel();
      throw new Error('Storage unavailable');
    }
    const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
    const cancel = () => { void reader.cancel().catch(() => {}); };
    signal.addEventListener('abort', cancel, { once: true });
    const prefix = `message,${name},`;
    let pending = '';
    try {
      while (!signal.aborted) {
        const part = await reader.read();
        if (part.done) return;
        pending += part.value;
        let end: number;
        while ((end = pending.indexOf('\n')) >= 0) {
          const line = pending.slice(0, end).replace(/\r$/, '');
          pending = pending.slice(end + 1);
          const data = line.startsWith('data:') ? line.slice(5).trimStart() : '';
          if (data.startsWith(prefix)) yield data.slice(prefix.length);
        }
        // A line that never ends is not a message this service sent.
        if (pending.length > MAX_MESSAGE_BYTES * 2) return;
      }
    } finally {
      signal.removeEventListener('abort', cancel);
      cancel();
    }
  }
  async limit(key: string, maximum: number, seconds: number, signal?: AbortSignal) {
    const result = await this.command(['EVAL', LIMIT, 1, `${this.prefix}:limit:${key}`, seconds], signal);
    if (typeof result !== 'number' || !Number.isSafeInteger(result) || result < 1) throw new Error('Storage unavailable');
    return result <= maximum;
  }
}

export function configuredStorage(env = process.env): Storage {
  const kv = !!(env.KV_REST_API_URL || env.KV_REST_API_TOKEN);
  const url = kv ? env.KV_REST_API_URL : env.UPSTASH_REDIS_REST_URL;
  const token = kv ? env.KV_REST_API_TOKEN : env.UPSTASH_REDIS_REST_TOKEN;
  if (url && token) return new RedisStorage(url, token);
  // Never silently deploy an ephemeral feed across independent instances.
  if (env.VERCEL || env.NODE_ENV === 'production' || url || token) throw new Error('Configure Redis REST storage');
  return new MemoryStorage();
}

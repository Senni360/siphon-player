import { httpsUrl, sourceUrl } from './urls.ts';

export interface User { user_id: string; display_name: string; secret_hash: string }
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
  author_id: string;
  created_at: number;
  likes: string[];
  replies?: Reply[];
}
export interface State { revision: number; users: User[]; posts: Post[] }
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
      new Set(state.users.map(u => u.secret_hash)).size !== state.users.length) {
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
      (p.to === undefined || Array.isArray(p.to) && p.to.length <= MAX_RECIPIENTS && new Set(p.to).size === p.to.length && p.to.every(id)) &&
      (p.replies === undefined || Array.isArray(p.replies) && p.replies.length <= MAX_REPLIES &&
        p.replies.every(r => r && id(r.id) && id(r.author_id) && Number.isSafeInteger(r.created_at) && r.created_at >= 0 && text(r.text, 500)) &&
        new Set(p.replies.map(r => r.id)).size === p.replies.length)) ||
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
}

export class MemoryStorage implements Storage {
  private state = empty();
  private limits = new Map<string, { count: number; until: number }>();
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

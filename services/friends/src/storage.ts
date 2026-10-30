export interface User { user_id: string; display_name: string; secret_hash: string }
export interface Payload {
  url: string;
  source: 'spotify' | 'soundcloud' | 'youtube';
  title: string;
  artists: string[];
  artwork_url: string | null;
  duration_ms: number;
  note?: string;
}
export interface Post extends Payload {
  id: string;
  author_id: string;
  created_at: number;
  likes: string[];
}
export interface State { revision: number; users: User[]; posts: Post[] }
export const empty = (): State => ({ revision: 0, users: [], posts: [] });

function storedState(value: unknown): State {
  if (!value || typeof value !== 'object') throw new Error('Invalid stored group');
  const state = value as State;
  if (!Number.isSafeInteger(state.revision) || state.revision < 0 ||
      !Array.isArray(state.users) || !Array.isArray(state.posts) ||
      !state.users.every(u => u && typeof u.user_id === 'string' && typeof u.display_name === 'string' && typeof u.secret_hash === 'string') ||
      !state.posts.every(p => p && typeof p.id === 'string' && typeof p.author_id === 'string' && Array.isArray(p.likes))) {
    throw new Error('Invalid stored group');
  }
  return state;
}

// Optimistic transactions keep joins, deletes and hearts atomic across Vercel
// instances. The group is deliberately small; one document keeps the adapter
// dependency-free. No read/modify/write is allowed without compareAndSet.
export interface Storage {
  read(): Promise<State>;
  compareAndSet(previous: number, next: State): Promise<boolean>;
  limit(key: string, maximum: number, seconds: number): Promise<boolean>;
}

export class MemoryStorage implements Storage {
  private state = empty();
  private limits = new Map<string, { count: number; until: number }>();
  async read() { return structuredClone(this.state); }
  async compareAndSet(previous: number, next: State) {
    if (this.state.revision !== previous) return false;
    this.state = structuredClone(next);
    return true;
  }
  async limit(key: string, maximum: number, seconds: number) {
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
  private async command(args: (string | number)[]): Promise<unknown> {
    const response = await fetch(this.url, {
      method: 'POST', headers: { Authorization: `Bearer ${this.token}`, 'Content-Type': 'application/json' },
      body: JSON.stringify(args), signal: AbortSignal.timeout(10000), redirect: 'error',
    });
    if (!response.ok) throw new Error('Storage unavailable');
    const value = await response.json() as { result?: unknown; error?: unknown } | null;
    if (!value || typeof value !== 'object' || 'error' in value || !('result' in value)) throw new Error('Storage unavailable');
    return value.result;
  }
  async read(): Promise<State> {
    const result = await this.command(['GET', `${this.prefix}:state`]);
    if (result === null) return empty();
    if (typeof result !== 'string') throw new Error('Invalid stored group');
    return storedState(JSON.parse(result));
  }
  async compareAndSet(previous: number, next: State) {
    const result = await this.command(['EVAL', CAS, 1, `${this.prefix}:state`, previous, JSON.stringify(next)]);
    if (result !== 0 && result !== 1) throw new Error('Storage unavailable');
    return result === 1;
  }
  async limit(key: string, maximum: number, seconds: number) {
    const result = await this.command(['EVAL', LIMIT, 1, `${this.prefix}:limit:${key}`, seconds]);
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

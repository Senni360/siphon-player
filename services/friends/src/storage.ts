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
    if (new URL(url).protocol !== 'https:') throw new Error('Redis REST requires HTTPS');
  }
  private async command(args: (string | number)[]): Promise<unknown> {
    const response = await fetch(this.url, {
      method: 'POST', headers: { Authorization: `Bearer ${this.token}`, 'Content-Type': 'application/json' },
      body: JSON.stringify(args), signal: AbortSignal.timeout(10000), redirect: 'error',
    });
    if (!response.ok) throw new Error('Storage unavailable');
    const value = await response.json() as { result?: unknown; error?: unknown };
    if (value.error) throw new Error('Storage unavailable');
    return value.result;
  }
  async read(): Promise<State> {
    const result = await this.command(['GET', `${this.prefix}:state`]);
    return typeof result === 'string' ? JSON.parse(result) as State : empty();
  }
  async compareAndSet(previous: number, next: State) {
    return await this.command(['EVAL', CAS, 1, `${this.prefix}:state`, previous, JSON.stringify(next)]) === 1;
  }
  async limit(key: string, maximum: number, seconds: number) {
    return Number(await this.command(['EVAL', LIMIT, 1, `${this.prefix}:limit:${key}`, seconds])) <= maximum;
  }
}

export function configuredStorage(env = process.env): Storage {
  const url = env.KV_REST_API_URL || env.UPSTASH_REDIS_REST_URL;
  const token = env.KV_REST_API_TOKEN || env.UPSTASH_REDIS_REST_TOKEN;
  if (url && token) return new RedisStorage(url, token);
  // Never silently deploy an ephemeral feed across independent instances.
  if (env.VERCEL || env.NODE_ENV === 'production' || url || token) throw new Error('Configure Redis REST storage');
  return new MemoryStorage();
}

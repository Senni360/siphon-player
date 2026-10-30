import { createHash, randomBytes, randomUUID, timingSafeEqual } from 'node:crypto';
import type { Payload, State, Storage, User } from './storage.ts';

export const hash = (text: string) => createHash('sha256').update(text).digest('hex');
class HttpError extends Error { constructor(public status: number, message: string) { super(message); } }
const fail = (status: number, message: string): never => { throw new HttpError(status, message); };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), {
  status, headers: { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' },
});
function text(value: unknown, name: string, max: number, optional = false): string {
  if (optional && (value === undefined || value === null)) return '';
  if (typeof value !== 'string' || (!optional && !value.trim()) || value.length > max || /[\x00-\x1f\x7f]/.test(value)) fail(400, `Invalid ${name}`);
  return (value as string).trim();
}
function https(value: unknown): URL {
  const raw = text(value, 'URL', 4096);
  let url: URL;
  try { url = new URL(raw); } catch { return fail(400, 'Invalid URL'); }
  if (url.protocol !== 'https:' || url.username || url.password || url.port || /%0[ad]/i.test(raw)) fail(400, 'Invalid URL');
  return url;
}
export function validateUrl(value: unknown, source: unknown): string {
  const url = https(value);
  const parts = url.pathname.split('/').filter(Boolean);
  const id = (value?: string) => !!value && /^[a-zA-Z0-9_-]+$/.test(value);
  let valid = false;
  if (source === 'spotify') {
    if (parts[0]?.startsWith('intl-')) parts.shift();
    valid = url.hostname === 'open.spotify.com' && parts.length === 2 && ['track', 'album', 'playlist'].includes(parts[0]) && /^[a-zA-Z0-9]+$/.test(parts[1]);
  } else if (source === 'soundcloud') {
    const reserved = ['likes', 'reposts', 'tracks', 'albums', 'sets', 'popular-tracks'];
    valid = url.hostname === 'on.soundcloud.com' ? parts.length === 1 && id(parts[0]) :
      ['soundcloud.com', 'www.soundcloud.com', 'm.soundcloud.com'].includes(url.hostname) &&
      parts.every(id) && !reserved.includes(parts[1]) && (parts.length === 2 || parts.length === 3 && /^s-[\w-]+$/.test(parts[2]));
    if (['soundcloud.com', 'www.soundcloud.com', 'm.soundcloud.com'].includes(url.hostname) && parts[1] === 'sets') {
      valid = parts.every(id) && (parts.length === 3 || parts.length === 4 && /^s-[\w-]+$/.test(parts[3]));
    }
  } else if (source === 'youtube') {
    const video = url.hostname === 'youtu.be' && parts.length === 1 ? parts[0] :
      ['youtube.com', 'www.youtube.com', 'music.youtube.com'].includes(url.hostname) && url.pathname === '/watch' ? url.searchParams.get('v') : null;
    valid = typeof video === 'string' && /^[\w-]{11}$/.test(video);
  }
  if (!valid) fail(400, 'URL does not match a supported source and playable item');
  url.hash = '';
  return url.toString();
}
function payload(body: Record<string, unknown>): Payload {
  const source = body.source as Payload['source'];
  const url = validateUrl(body.url, source);
  if (!Array.isArray(body.artists) || body.artists.length > 20) fail(400, 'Invalid artists');
  const artists = (body.artists as unknown[]).map(a => text(a, 'artist', 200));
  if (!Number.isSafeInteger(body.duration_ms) || Number(body.duration_ms) < 0 || Number(body.duration_ms) > 604800000) fail(400, 'Invalid duration_ms');
  const artwork = body.artwork_url == null || body.artwork_url === '' ? null : https(body.artwork_url).toString();
  return { url, source, title: text(body.title, 'title', 300), artists, artwork_url: artwork, duration_ms: Number(body.duration_ms), note: text(body.note, 'note', 280, true) };
}
async function body(request: Request): Promise<Record<string, unknown>> {
  if (request.headers.get('content-type')?.split(';')[0].trim().toLowerCase() !== 'application/json') fail(415, 'Use application/json');
  if (Number(request.headers.get('content-length')) > 16384) fail(413, 'Request too large');
  const reader = request.body?.getReader();
  if (!reader) return fail(400, 'Expected JSON object');
  let size = 0;
  const chunks: Uint8Array[] = [];
  while (true) {
    const part = await reader.read();
    if (part.done) break;
    size += part.value.length;
    if (size > 16384) { await reader.cancel(); return fail(413, 'Request too large'); }
    chunks.push(part.value);
  }
  try {
    const value: unknown = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    if (value && typeof value === 'object' && !Array.isArray(value)) return value as Record<string, unknown>;
  } catch { /* fixed error, never echo input */ }
  return fail(400, 'Expected JSON object');
}
async function change<T>(storage: Storage, update: (state: State) => T): Promise<T> {
  for (let i = 0; i < 20; i++) {
    const state = await storage.read();
    const previous = state.revision;
    const result = update(state);
    state.revision++;
    if (await storage.compareAndSet(previous, state)) return result;
  }
  return fail(503, 'Feed busy, try again');
}

function member(state: State, secretHash: string): User {
  return state.users.find(user => user.secret_hash === secretHash) ?? fail(401, 'Sign in to the group');
}

function memberChange<T>(storage: Storage, secretHash: string, update: (state: State, user: User) => T): Promise<T> {
  // Authorization must still hold in the revision we commit. A request that
  // started before membership revocation cannot write after it, even on retry.
  return change(storage, state => update(state, member(state, secretHash)));
}

export function createService(storage: Storage, inviteCode: string) {
  return async (request: Request, clientIp = 'local'): Promise<Response> => {
    try {
      if (inviteCode.length < 12 || inviteCode.length > 256 || inviteCode.trim() !== inviteCode || /[\x00-\x1f\x7f]/.test(inviteCode)) return fail(503, 'Group invite is not configured');
      const path = new URL(request.url).pathname.replace(/^\/api/, '').replace(/\/$/, '');
      const method = request.method;
      // IP identifiers are hashed before storage. On Vercel the adapter supplies
      // only the platform-overwritten x-vercel-forwarded-for, never a client header.
      if (!await storage.limit(`ip:${hash(clientIp)}`, 180, 60)) return fail(429, 'Too many requests');
      if (path === '/join' && method === 'POST') {
        if (!await storage.limit(`join:${hash(clientIp)}`, 10, 3600)) return fail(429, 'Too many join attempts');
        const input = await body(request);
        const supplied = text(input.invite_code, 'invite_code', 256);
        if (!timingSafeEqual(Buffer.from(hash(supplied)), Buffer.from(hash(inviteCode)))) return fail(403, 'Invalid invite code');
        const display_name = text(input.display_name, 'display_name', 40);
        const secret = randomBytes(32).toString('hex');
        const user_id = randomUUID();
        await change(storage, state => {
          if (state.users.length >= 500) fail(409, 'Group is full');
          state.users.push({ user_id, display_name, secret_hash: hash(secret) });
        });
        return json({ user_id, secret }, 201);
      }
      const bearer = request.headers.get('authorization');
      if (!bearer || !/^Bearer [a-f0-9]{64}$/.test(bearer)) return fail(401, 'Sign in to the group');
      const secret_hash = hash(bearer.slice(7));
      const state = await storage.read();
      const user = member(state, secret_hash);
      const reads = method === 'GET';
      if (!await storage.limit(`user:${user.user_id}:${reads ? 'read' : 'write'}`, reads ? 120 : 60, 60)) return fail(429, 'Too many requests');
      if (path === '/me' && method === 'GET') return json({ user_id: user.user_id, display_name: user.display_name });
      if (path === '/me' && method === 'PATCH') {
        const display_name = text((await body(request)).display_name, 'display_name', 40);
        await memberChange(storage, secret_hash, (_s, current) => { current.display_name = display_name; });
        return json({ user_id: user.user_id, display_name });
      }
      if (path === '/me' && method === 'DELETE') {
        await memberChange(storage, secret_hash, (s, current) => {
          s.users = s.users.filter(u => u.user_id !== current.user_id);
          for (const post of s.posts) post.likes = post.likes.filter(id => id !== current.user_id);
        });
        return json({ ok: true });
      }
      if (path === '/posts' && method === 'GET') {
        const params = new URL(request.url).searchParams;
        const limit = Number(params.get('limit') ?? 30);
        if (!Number.isInteger(limit) || limit < 1 || limit > 100) return fail(400, 'Invalid limit');
        let cursor: { created_at: number; id: string } | undefined;
        if (params.has('cursor')) {
          try {
            const raw = params.get('cursor')!;
            if (raw.length > 256) throw new Error();
            cursor = JSON.parse(Buffer.from(raw, 'base64url').toString());
            if (!cursor || !Number.isSafeInteger(cursor.created_at) || typeof cursor.id !== 'string' || !/^[\w-]{36}$/.test(cursor.id)) throw new Error();
          } catch { return fail(400, 'Invalid cursor'); }
        }
        const posts = state.posts.filter(p => !cursor || p.created_at < cursor.created_at || p.created_at === cursor.created_at && p.id < cursor.id)
          .sort((a, b) => b.created_at - a.created_at || b.id.localeCompare(a.id));
        const page = posts.slice(0, limit);
        const last = page.at(-1);
        return json({ posts: page.map(({ likes, ...p }) => ({ ...p, author_name: state.users.find(u => u.user_id === p.author_id)?.display_name ?? 'Former member', like_count: likes.length, liked: likes.includes(user.user_id) })),
          next_cursor: posts.length > limit && last ? Buffer.from(JSON.stringify({ created_at: last.created_at, id: last.id })).toString('base64url') : null });
      }
      if (path === '/posts' && method === 'POST') {
        const input = payload(await body(request));
        const post = { ...input, id: randomUUID(), author_id: user.user_id, created_at: Date.now(), likes: [] as string[] };
        const author_name = await memberChange(storage, secret_hash, (s, current) => {
          if (s.posts.length >= 5000) fail(409, 'Feed is full, delete older posts');
          s.posts.push(post);
          return current.display_name;
        });
        const { likes, ...result } = post;
        return json({ ...result, author_name, like_count: likes.length, liked: false }, 201);
      }
      const match = /^\/posts\/([a-f0-9-]{36})(\/like)?$/.exec(path);
      if (match && (method === 'DELETE' || match[2] && method === 'PUT')) {
        await memberChange(storage, secret_hash, s => {
          const post = s.posts.find(p => p.id === match[1]);
          if (!post) return fail(404, 'Post not found');
          if (match[2]) {
            post.likes = post.likes.filter(id => id !== user.user_id);
            if (method === 'PUT') post.likes.push(user.user_id);
          } else {
            if (post.author_id !== user.user_id) return fail(403, 'You can only delete your own post');
            s.posts = s.posts.filter(p => p.id !== post.id);
          }
        });
        return json({ ok: true });
      }
      return fail(404, 'Route not found');
    } catch (error) {
      if (error instanceof HttpError) {
        const response = json({ error: error.message }, error.status);
        if (error.status === 429) response.headers.set('Retry-After', '60');
        return response;
      }
      // Storage and transport errors can include authorization headers. Never log them.
      return json({ error: 'Service unavailable' }, 503);
    }
  };
}

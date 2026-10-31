import { createHash, randomBytes, randomInt, randomUUID, timingSafeEqual } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import { MAX_POSTS, MAX_RECIPIENTS, MAX_REPLIES, MAX_USERS, StorageFullError } from './storage.js';
import { httpsUrl, sourceUrl } from './urls.js';
import type { Payload, Post, State, Storage, User } from './storage.js';
import { describe, noJev, titleReadings, type Ask, type Track } from './jev.js';

export const hash = (text: string) => createHash('sha256').update(text).digest('hex');
class HttpError extends Error { constructor(public status: number, message: string, public retryAfter?: number) { super(message); } }
const fail = (status: number, message: string, retryAfter?: number): never => { throw new HttpError(status, message, retryAfter); };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), {
  status, headers: { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' },
});
function text(value: unknown, name: string, max: number, optional = false): string {
  if (optional && (value === undefined || value === null)) return '';
  if (typeof value !== 'string' || (!optional && !value.trim()) || value.length > max || /[\x00-\x1f\x7f]/.test(value)) fail(400, `Invalid ${name}`);
  return (value as string).trim();
}
function https(value: unknown): URL {
  return httpsUrl(value) ?? fail(400, 'Invalid URL');
}
export function validateUrl(value: unknown, source: unknown): string {
  return sourceUrl(value, source) ?? fail(400, 'URL does not match a supported source and playable item');
}
function payload(body: Record<string, unknown>): Payload {
  const source = body.source as Payload['source'];
  const url = validateUrl(body.url, source);
  if (!Array.isArray(body.artists) || body.artists.length > 20) fail(400, 'Invalid artists');
  const artists = (body.artists as unknown[]).map(a => text(a, 'artist', 200));
  if (!Number.isSafeInteger(body.duration_ms) || Number(body.duration_ms) < 0 || Number(body.duration_ms) > 604800000) fail(400, 'Invalid duration_ms');
  const artwork = body.artwork_url == null || body.artwork_url === '' ? null : https(body.artwork_url).toString();
  const to = recipients(body.to);
  return { url, source, title: text(body.title, 'title', 300), artists, artwork_url: artwork, duration_ms: Number(body.duration_ms), note: text(body.note, 'note', 280, true), ...(to.length ? { to } : {}) };
}
const UUID = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;
/** Who a share goes to: absent or empty means the whole group. */
function recipients(value: unknown): string[] {
  if (value === undefined || value === null) return [];
  if (!Array.isArray(value) || value.length > MAX_RECIPIENTS || !value.every(id => typeof id === 'string' && UUID.test(id)) || new Set(value).size !== value.length) fail(400, 'Invalid recipients');
  return value as string[];
}
/** A share sent to some members is seen only by them and its author. */
export function visible(post: Post, reader: string): boolean {
  return !post.to?.length || post.author_id === reader || post.to.includes(reader);
}
async function body(request: Request, signal: AbortSignal, max = 16384): Promise<Record<string, unknown>> {
  if (request.headers.get('content-type')?.split(';')[0].trim().toLowerCase() !== 'application/json') fail(415, 'Use application/json');
  if (Number(request.headers.get('content-length')) > max) fail(413, 'Request too large');
  const reader = request.body?.getReader();
  if (!reader) return fail(400, 'Expected JSON object');
  let size = 0;
  const chunks: Uint8Array[] = [];
  const cancel = () => { void reader.cancel().catch(() => {}); };
  signal.addEventListener('abort', cancel, { once: true });
  try {
    signal.throwIfAborted();
    while (true) {
      const part = await reader.read();
      signal.throwIfAborted();
      if (part.done) break;
      size += part.value.length;
      if (size > max) { await reader.cancel(); return fail(413, 'Request too large'); }
      chunks.push(part.value);
    }
  } finally { signal.removeEventListener('abort', cancel); reader.releaseLock(); }
  try {
    const value: unknown = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    if (value && typeof value === 'object' && !Array.isArray(value)) return value as Record<string, unknown>;
  } catch { /* fixed error, never echo input */ }
  return fail(400, 'Expected JSON object');
}
async function change<T>(storage: Storage, update: (state: State) => T, signal: AbortSignal): Promise<T> {
  for (let i = 0; i < 20; i++) {
    signal.throwIfAborted();
    const state = await storage.read(signal);
    signal.throwIfAborted();
    const previous = state.revision;
    const result = update(state);
    state.revision++;
    if (await storage.compareAndSet(previous, state, signal)) return result;
    // Spread colliding writers instead of repeating the same synchronized
    // read/write wave. Storage exceptions are uncertain, so never retry them.
    if (i < 19) await delay(randomInt(1, Math.min(2 ** (i + 2), 65)), undefined, { signal });
  }
  return fail(503, 'Feed busy, try again');
}

function member(state: State, secretHash: string): User {
  return state.users.find(user => user.secret_hash === secretHash) ?? fail(401, 'Sign in to the group');
}

function memberChange<T>(storage: Storage, secretHash: string, update: (state: State, user: User) => T, signal: AbortSignal): Promise<T> {
  // Authorization must still hold in the revision we commit. A request that
  // started before membership revocation cannot write after it, even on retry.
  return change(storage, state => update(state, member(state, secretHash)), signal);
}

function track(value: unknown, name: string): Track {
  if (!value || typeof value !== 'object') return fail(400, `Invalid ${name}`);
  const raw = value as Record<string, unknown>;
  if (!Array.isArray(raw.artists) || raw.artists.length > 5) fail(400, `Invalid ${name}`);
  return { title: text(raw.title, 'title', 300), artists: (raw.artists as unknown[]).map(a => text(a, 'artist', 200)) };
}
function list(value: unknown, name: string, min: number, max: number): unknown[] {
  if (!Array.isArray(value) || value.length < min || value.length > max) fail(400, `Invalid ${name}`);
  return value as unknown[];
}
/** A member's taste: their own shares and hearts, newest first. */
function favourites(state: State, userId: string): string[] {
  return state.posts.filter(p => p.author_id === userId || p.likes.includes(userId))
    .sort((a, b) => b.created_at - a.created_at).slice(0, 15).map(describe);
}
// Jev runs on the maintainer's key for the whole group. Bound each member so
// a runaway client cannot spend it; a refusal reads as "no opinion".
async function jevAllowed(storage: Storage, userId: string, signal: AbortSignal): Promise<boolean> {
  return await storage.limit(`jev:${userId}:minute`, 30, 60, signal) && await storage.limit(`jev:${userId}:day`, 3000, 86400, signal);
}

export function createService(storage: Storage, inviteCode: string, ask: Ask = noJev) {
  /**
   * Scores shares the reader has not rated yet against their taste, once per
   * share and reader. The scores are a cache: losing a write only means
   * asking again on the next read.
   */
  async function affinities(state: State, user: User, page: Post[], headers: Headers, signal: AbortSignal): Promise<Map<string, number>> {
    const pending = page.filter(p => p.author_id !== user.user_id && p.affinity?.[user.user_id] === undefined).slice(0, 24);
    const taste = favourites(state, user.user_id);
    if (!pending.length || taste.length < 2 || !await jevAllowed(storage, user.user_id, signal)) return new Map();
    const answers = await ask(`A listener's recent favourites, newest first:\n${taste.join('\n')}`,
      Object.fromEntries(pending.map((p, i) => [`p${i}`, { type: 'noul' as const, instructions: `Would this listener enjoy ${describe(p)}?` }])), headers, signal);
    const scores = new Map<string, number>();
    pending.forEach((p, i) => {
      const answer = answers?.[`p${i}`];
      if (answer?.type === 'noul') scores.set(p.id, Math.round(answer.probability * 100) / 100);
    });
    if (scores.size) {
      try {
        await change(storage, s => {
          if (!s.users.some(u => u.user_id === user.user_id)) return;
          for (const post of s.posts) {
            const score = scores.get(post.id);
            if (score !== undefined) (post.affinity ??= {})[user.user_id] = score;
          }
        }, signal);
      } catch { /* a cache write; the reader still gets this answer */ }
    }
    return scores;
  }

  return async (request: Request, clientIp = 'local'): Promise<Response> => {
    try {
      const signal = AbortSignal.any([request.signal, AbortSignal.timeout(12000)]);
      signal.throwIfAborted();
      if (inviteCode.length < 12 || inviteCode.length > 256 || inviteCode.trim() !== inviteCode || /[\x00-\x1f\x7f]/.test(inviteCode)) return fail(503, 'Group invite is not configured');
      const path = new URL(request.url).pathname.replace(/^\/api/, '').replace(/\/$/, '');
      const method = request.method;
      // IP identifiers are hashed before storage. On Vercel the adapter supplies
      // only the platform-overwritten x-vercel-forwarded-for, never a client header.
      if (!await storage.limit(`ip:${hash(clientIp)}`, 180, 60, signal)) return fail(429, 'Too many requests', 60);
      if (path === '/join' && method === 'POST') {
        if (!await storage.limit(`join:${hash(clientIp)}`, 10, 3600, signal)) return fail(429, 'Too many join attempts', 3600);
        const input = await body(request, signal);
        const supplied = text(input.invite_code, 'invite_code', 256);
        if (!timingSafeEqual(Buffer.from(hash(supplied)), Buffer.from(hash(inviteCode)))) return fail(403, 'Invalid invite code');
        const display_name = text(input.display_name, 'display_name', 40);
        const secret = randomBytes(32).toString('hex');
        const user_id = randomUUID();
        await change(storage, state => {
          if (state.users.length >= MAX_USERS) fail(409, 'Group is full');
          state.users.push({ user_id, display_name, secret_hash: hash(secret) });
        }, signal);
        return json({ user_id, secret }, 201);
      }
      const bearer = request.headers.get('authorization');
      if (!bearer || !/^Bearer [a-f0-9]{64}$/.test(bearer)) return fail(401, 'Sign in to the group');
      const secret_hash = hash(bearer.slice(7));
      const state = await storage.read(signal);
      const user = member(state, secret_hash);
      const reads = method === 'GET';
      if (!await storage.limit(`user:${user.user_id}:${reads ? 'read' : 'write'}`, reads ? 120 : 60, 60, signal)) return fail(429, 'Too many requests', 60);
      signal.throwIfAborted();
      if (path === '/me' && method === 'GET') return json({ user_id: user.user_id, display_name: user.display_name });
      if (path === '/me' && method === 'PATCH') {
        const display_name = text((await body(request, signal)).display_name, 'display_name', 40);
        await memberChange(storage, secret_hash, (_s, current) => { current.display_name = display_name; }, signal);
        return json({ user_id: user.user_id, display_name });
      }
      if (path === '/me' && method === 'DELETE') {
        await memberChange(storage, secret_hash, (s, current) => {
          s.users = s.users.filter(u => u.user_id !== current.user_id);
          for (const post of s.posts) {
            post.likes = post.likes.filter(id => id !== current.user_id);
            if (post.affinity) delete post.affinity[current.user_id];
          }
        }, signal);
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
        const posts = state.posts.filter(p => visible(p, user.user_id)).filter(p => !cursor || p.created_at < cursor.created_at || p.created_at === cursor.created_at && p.id < cursor.id)
          .sort((a, b) => b.created_at - a.created_at || b.id.localeCompare(a.id));
        const page = posts.slice(0, limit);
        const last = page.at(-1);
        const authors = new Map(state.users.map(u => [u.user_id, u.display_name]));
        const scored = await affinities(state, user, page, request.headers, signal);
        return json({ posts: page.map(p => ({
          id: p.id, author_id: p.author_id, created_at: p.created_at,
          url: p.url, source: p.source, title: p.title, artists: p.artists,
          artwork_url: p.artwork_url, duration_ms: p.duration_ms, note: p.note ?? '',
          author_name: authors.get(p.author_id) ?? 'Former member', like_count: p.likes.length, liked: p.likes.includes(user.user_id),
          to: p.to ?? [],
          affinity: scored.get(p.id) ?? p.affinity?.[user.user_id] ?? null,
          replies: (p.replies ?? []).map(r => ({ id: r.id, author_id: r.author_id, author_name: authors.get(r.author_id) ?? 'Former member', created_at: r.created_at, text: r.text })),
        })),
          next_cursor: posts.length > limit && last ? Buffer.from(JSON.stringify({ created_at: last.created_at, id: last.id })).toString('base64url') : null });
      }
      if (path === '/members' && method === 'GET') {
        return json({ members: state.users.map(u => ({ user_id: u.user_id, display_name: u.display_name })) });
      }
      if (path === '/posts' && method === 'POST') {
        const input = payload(await body(request, signal));
        const post = { ...input, id: randomUUID(), author_id: user.user_id, created_at: Date.now(), likes: [] as string[] };
        const author_name = await memberChange(storage, secret_hash, (s, current) => {
          if (s.posts.length >= MAX_POSTS) fail(409, 'Feed is full, delete older posts');
          // Recipients must be members when the share is sent.
          if (post.to?.some(id => !s.users.some(u => u.user_id === id))) fail(400, 'Invalid recipients');
          s.posts.push(post);
          return current.display_name;
        }, signal);
        const { likes, ...result } = post;
        return json({ ...result, to: post.to ?? [], replies: [], author_name, like_count: likes.length, liked: false }, 201);
      }
      if (path === '/jev/next' && method === 'POST') {
        // Which candidate best continues what just played. The player uses
        // the order to fill autoplay and to warm the likeliest next song.
        const input = await body(request, signal, 131072);
        const recent = list(input.recent, 'recent', 1, 10).map(t => track(t, 'recent'));
        const candidates = list(input.candidates, 'candidates', 2, 255).map(c => {
          const id = (c as Record<string, unknown> | null)?.id;
          if (typeof id !== 'string' || !/^[\w:.-]{1,80}$/.test(id)) fail(400, 'Invalid candidates');
          return { id: id as string, ...track(c, 'candidates') };
        });
        if (new Set(candidates.map(c => c.id)).size !== candidates.length) fail(400, 'Invalid candidates');
        if (!await jevAllowed(storage, user.user_id, signal)) return json({ probabilities: null });
        const answers = await ask(`Songs just played in this listening session, newest first:\n${recent.map(describe).join('\n')}`, {
          next: { type: 'choice', instructions: 'Which song should play next so the session keeps its flow?', criteria: Object.fromEntries(candidates.map(c => [c.id, describe(c)])) },
        }, request.headers, signal);
        const next = answers?.next;
        return json({ probabilities: next?.type === 'choice' && Object.keys(next.probabilities).length ? next.probabilities : null });
      }
      if (path === '/jev/titles' && method === 'POST') {
        // Splits messy upload titles into artist and song. Jev only picks
        // between readings made here; a weak pick leaves the title alone.
        const uploads = list((await body(request, signal, 65536)).uploads, 'uploads', 1, 50).map(u => {
          const raw = (u && typeof u === 'object' ? u : {}) as Record<string, unknown>;
          return { title: text(raw.title, 'title', 300), uploader: text(raw.uploader, 'uploader', 200, true) };
        });
        const readings = uploads.map(u => titleReadings(u.title, u.uploader));
        const open = readings.flatMap((r, i) => r.length > 1 ? [i] : []);
        const answers = open.length && await jevAllowed(storage, user.user_id, signal) ? await ask('Uploads on a music site, by title and uploader:\n' +
          open.map(i => `u${i}: "${uploads[i].title}" uploaded by "${uploads[i].uploader || 'unknown'}"`).join('\n'),
          Object.fromEntries(open.map(i => [`u${i}`, { type: 'choice' as const, instructions: `For upload u${i}, which reading names its performing artist and song title correctly?`,
            criteria: Object.fromEntries(readings[i].map((r, j) => [`r${j}`, `Artist: ${r.artist}; Song: ${r.title}`])) }])), request.headers, signal) : null;
        return json({ readings: readings.map((r, i) => {
          if (r.length < 2) return r[0] ?? null;
          const answer = answers?.[`u${i}`];
          return answer?.type === 'choice' && (answer.probabilities[answer.choice] ?? 0) >= 0.6 ? r[Number(answer.choice.slice(1))] ?? null : null;
        }) });
      }
      const replies = /^\/posts\/([a-f0-9-]{36})\/replies(?:\/([a-f0-9-]{36}))?$/.exec(path);
      if (replies && method === 'POST' && !replies[2]) {
        const reply_text = text((await body(request, signal)).text, 'text', 500);
        const reply = { id: randomUUID(), author_id: user.user_id, created_at: Date.now(), text: reply_text };
        const author_name = await memberChange(storage, secret_hash, (s, current) => {
          const post = s.posts.find(p => p.id === replies[1]);
          if (!post || !visible(post, current.user_id)) return fail(404, 'Post not found');
          post.replies ??= [];
          if (post.replies.length >= MAX_REPLIES) fail(409, 'This thread is full');
          post.replies.push(reply);
          return current.display_name;
        }, signal);
        return json({ ...reply, author_name }, 201);
      }
      if (replies && method === 'DELETE' && replies[2]) {
        await memberChange(storage, secret_hash, (s, current) => {
          const post = s.posts.find(p => p.id === replies[1]);
          if (!post || !visible(post, current.user_id)) return fail(404, 'Post not found');
          const reply = post.replies?.find(r => r.id === replies[2]);
          if (!reply) return fail(404, 'Reply not found');
          if (reply.author_id !== current.user_id) return fail(403, 'You can only delete your own reply');
          post.replies = post.replies!.filter(r => r.id !== reply.id);
        }, signal);
        return json({ ok: true });
      }
      const match = /^\/posts\/([a-f0-9-]{36})(\/like)?$/.exec(path);
      if (match && (method === 'DELETE' || match[2] && method === 'PUT')) {
        await memberChange(storage, secret_hash, s => {
          const post = s.posts.find(p => p.id === match[1]);
          if (!post || !visible(post, user.user_id)) return fail(404, 'Post not found');
          if (match[2]) {
            post.likes = post.likes.filter(id => id !== user.user_id);
            if (method === 'PUT') post.likes.push(user.user_id);
          } else {
            if (post.author_id !== user.user_id) return fail(403, 'You can only delete your own post');
            s.posts = s.posts.filter(p => p.id !== post.id);
          }
        }, signal);
        return json({ ok: true });
      }
      return fail(404, 'Route not found');
    } catch (error) {
      if (error instanceof HttpError) {
        const response = json({ error: error.message }, error.status);
        if (error.retryAfter) response.headers.set('Retry-After', String(error.retryAfter));
        return response;
      }
      if (error instanceof StorageFullError) return json({ error: 'Feed storage is full, delete older posts' }, 409);
      // Storage and transport errors can include authorization headers. Never log them.
      return json({ error: 'Service unavailable' }, 503);
    }
  };
}

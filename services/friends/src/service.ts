import { createHash, randomBytes, randomInt, randomUUID, timingSafeEqual } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import { MAX_POSTS, MAX_RECIPIENTS, MAX_REPLIES, MAX_USERS, StorageFullError } from './storage.js';
import { HttpError, body, fail, https, json, text, validateUrl } from './http.js';
import { createLive, song } from './live.js';
import { FIRST_GROUP, MAX_DEVICES, admitPending, fullRights, groupById, groupRoute, manages, memberOf, memberRights, mergeUsers, removeMember, requestGroup, withGroups } from './groups.js';
import type { Payload, Post, SharedPlaylist, State, Storage, User } from './storage.js';
import { describe, noJev, titleReadings, type Ask, type Track } from './jev.js';

export const hash = (text: string) => createHash('sha256').update(text).digest('hex');
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

const holds = (user: User, secretHash: string) => user.secret_hash === secretHash || !!user.devices?.some(d => d.hash === secretHash);

function member(state: State, secretHash: string): User {
  return state.users.find(user => holds(user, secretHash)) ?? fail(401, 'Sign in to the group');
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

// A live read comes every few seconds while listening together, so it must not
// cost the group's whole document each time. Who belongs is remembered briefly
// per instance; a removed member keeps live access for at most this long.
const ROSTER_MS = 60000;
const LIVE_READS_PER_MINUTE = 120;
const LIVE_WRITES_PER_MINUTE = 60;

export { validateUrl };

/** Who a Spotify token belongs to. The token is used for this one read and goes nowhere else. */
export type VerifySpotify = (token: string, signal: AbortSignal) => Promise<{ id: string; display_name: string | null }>;
export const verifySpotify: VerifySpotify = async (token, signal) => {
  const response = await fetch('https://api.spotify.com/v1/me', { headers: { Authorization: `Bearer ${token}` }, redirect: 'error', signal }).catch(() => fail(502, 'Spotify could not be reached'));
  if (response.status === 401) return fail(401, 'Spotify did not accept this sign-in');
  if (!response.ok) return fail(502, 'Spotify could not be reached');
  const profile = await response.json().catch(() => null) as { id?: unknown; display_name?: unknown } | null;
  if (typeof profile?.id !== 'string' || !profile.id) return fail(502, 'Spotify could not be reached');
  return { id: profile.id, display_name: typeof profile.display_name === 'string' ? profile.display_name : null };
};
const playlistKey = (id: string) => `playlist:${id}`;
const PLAYLIST_SECONDS = 400 * 86400;
const MAX_PLAYLISTS = 1000;
const MAX_PLAYLIST_SONGS = 1000;
const seenByReader = (list: SharedPlaylist, reader: string) => !list.to?.length || list.by === reader || list.to.includes(reader);

export function createService(storage: Storage, inviteCode: string, ask: Ask = noJev, clock: () => number = Date.now, verify: VerifySpotify = verifySpotify, ownerSpotify = '') {
  const live = createLive(storage, clock);
  let roster: { state: State; until: number } | null = null;
  const liveCalls = new Map<string, { count: number; until: number }>();
  function liveAllowed(key: string, maximum: number): boolean {
    const now = clock();
    for (const [name, value] of liveCalls) if (value.until <= now) liveCalls.delete(name);
    const bucket = liveCalls.get(key) ?? { count: 0, until: now + 60000 };
    liveCalls.set(key, bucket);
    return ++bucket.count <= maximum;
  }
  async function liveRoute(request: Request, path: string, clientIp: string, signal: AbortSignal): Promise<Response> {
    const bearer = request.headers.get('authorization');
    if (!bearer || !/^Bearer [a-f0-9]{64}$/.test(bearer)) return fail(401, 'Sign in to the group');
    const secret_hash = hash(bearer.slice(7));
    let user = roster && roster.until > clock() ? roster.state.users.find(u => holds(u, secret_hash)) : undefined;
    if (!user) {
      // Unknown here: as expensive as any other request, and limited as one.
      if (!await storage.limit(`ip:${hash(clientIp)}`, 180, 60, signal)) return fail(429, 'Too many requests', 60);
      const state = await storage.read(signal);
      roster = { state, until: clock() + ROSTER_MS };
      user = member(state, secret_hash);
    }
    const reads = request.method === 'GET';
    if (!liveAllowed(`${user.user_id}:${reads ? 'read' : 'write'}`, reads ? LIVE_READS_PER_MINUTE : LIVE_WRITES_PER_MINUTE)) return fail(429, 'Too many requests', 60);
    const { group, member: seat } = requestGroup(roster!.state, request.headers.get('x-siphon-group'), user);
    const here = new Set(group.members.map(m => m.user_id));
    return await live.route(request, path, user, roster!.state.users.filter(u => here.has(u.user_id)), signal, group.id, seat.rights) ?? fail(404, 'Route not found');
  }

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
      // A Discord button can only open https. This names no song and no person,
      // and the app only lets in those already in the group.
      const door = /^\/open\/party\/([a-f0-9-]{36})\/([a-f0-9-]{36})$/.exec(path);
      if (door && method === 'GET') return new Response(null, { status: 302, headers: { Location: `siphon://party/${door[1]}/${door[2]}`, 'Cache-Control': 'no-store' } });
      if (path === '/live' || path.startsWith('/live/')) return await liveRoute(request, path, clientIp, signal);
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
          // Before the push: a document from before groups gives everyone so far a seat.
          withGroups(state);
          state.users.push({ user_id, display_name, secret_hash: hash(secret) });
          groupById(state, FIRST_GROUP)?.members.push({ user_id, role: 'member', rights: memberRights() });
        }, signal);
        return json({ user_id, secret }, 201);
      }
      if (path === '/auth/spotify' && method === 'POST') {
        // An existing identity may come along: that person's first sign-in on
        // this account, or a second identity of theirs to fold into it.
        if (!await storage.limit(`auth:${hash(clientIp)}`, 30, 3600, signal)) return fail(429, 'Too many sign-ins', 3600);
        const input = await body(request, signal, 8192);
        const token = text(input.token, 'token', 4096);
        if (!/^[\w.~+/=-]+$/.test(token)) return fail(400, 'Invalid token');
        const device = text(input.device, 'device', 60, true) || 'This computer';
        const held = request.headers.get('authorization');
        const heldHash = held && /^Bearer [a-f0-9]{64}$/.test(held) ? hash(held.slice(7)) : null;
        const profile = await verify(token, signal);
        const secret = randomBytes(32).toString('hex');
        const answer = await change(storage, state => {
          withGroups(state);
          const holder = heldHash ? state.users.find(u => holds(u, heldHash)) : undefined;
          let account = state.users.find(u => u.spotify_id === profile.id);
          let created = false;
          if (holder && account && holder !== account) mergeUsers(state, holder, account);
          else if (holder && !account) { holder.spotify_id = profile.id; account = holder; }
          else if (!account) {
            if (state.users.length >= MAX_USERS) fail(409, 'Group is full');
            const name = (profile.display_name ?? '').replace(/[\x00-\x1f\x7f]/g, '').trim().slice(0, 40).trim() || 'Listener';
            account = { user_id: randomUUID(), display_name: name, secret_hash: hash(randomBytes(32).toString('hex')), spotify_id: profile.id };
            state.users.push(account);
            created = true;
          }
          account.devices = [...account.devices ?? [], { id: randomUUID(), hash: hash(secret), name: device, created_at: clock() }].slice(-MAX_DEVICES);
          admitPending(state, account);
          // The owner the deployment names takes the first group once they prove who they are.
          const first = groupById(state, FIRST_GROUP);
          if (first && ownerSpotify && profile.id === ownerSpotify && first.owner_id !== account.user_id && memberOf(first, account.user_id)) {
            for (const seat of first.members) if (seat.role === 'owner') seat.role = 'admin';
            first.owner_id = account.user_id;
            const seat = memberOf(first, account.user_id)!;
            seat.role = 'owner';
            seat.rights = fullRights();
          }
          return { user_id: account.user_id, display_name: account.display_name, created };
        }, signal);
        roster = null;
        return json({ ...answer, secret }, 201);
      }
      const bearer = request.headers.get('authorization');
      if (!bearer || !/^Bearer [a-f0-9]{64}$/.test(bearer)) return fail(401, 'Sign in to the group');
      const secret_hash = hash(bearer.slice(7));
      const state = await storage.read(signal);
      const user = member(state, secret_hash);
      const reads = method === 'GET';
      if (!await storage.limit(`user:${user.user_id}:${reads ? 'read' : 'write'}`, reads ? 120 : 60, 60, signal)) return fail(429, 'Too many requests', 60);
      signal.throwIfAborted();
      const scoped = await groupRoute(request, path, user, state, secret_hash, update => memberChange(storage, secret_hash, update, signal).finally(() => { roster = null; }), signal);
      if (scoped) return scoped;
      if (path === '/me' && method === 'GET') return json({ user_id: user.user_id, display_name: user.display_name });
      if (path === '/me' && method === 'PATCH') {
        const display_name = text((await body(request, signal)).display_name, 'display_name', 40);
        await memberChange(storage, secret_hash, (_s, current) => { current.display_name = display_name; }, signal);
        return json({ user_id: user.user_id, display_name });
      }
      if (path === '/me' && method === 'DELETE') {
        await memberChange(storage, secret_hash, (s, current) => {
          for (const group of withGroups(s)) if (memberOf(group, current.user_id)) removeMember(s, group, current.user_id);
          s.users = s.users.filter(u => u.user_id !== current.user_id);
          for (const post of s.posts) {
            post.likes = post.likes.filter(id => id !== current.user_id);
            if (post.affinity) delete post.affinity[current.user_id];
          }
        }, signal);
        roster = null;
        // Everything live expires by itself; this only makes it immediate.
        await live.forget(user, state.users, signal).catch(() => {});
        return json({ ok: true });
      }
      // Everything below belongs to one group: the one named, or the first.
      const { group, member: seat } = requestGroup(state, request.headers.get('x-siphon-group'), user);
      const inGroup = (post: Post) => (post.group_id ?? FIRST_GROUP) === group.id;
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
        const posts = state.posts.filter(inGroup).filter(p => visible(p, user.user_id)).filter(p => !cursor || p.created_at < cursor.created_at || p.created_at === cursor.created_at && p.id < cursor.id)
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
        const names = new Map(state.users.map(u => [u.user_id, u.display_name]));
        return json({ members: group.members.map(m => ({ user_id: m.user_id, display_name: names.get(m.user_id) ?? 'Former member' })) });
      }
      if (path === '/posts' && method === 'POST') {
        if (!seat.rights.share) return fail(403, 'An admin turned off sharing for you in this group');
        const input = payload(await body(request, signal));
        const post: Post = { ...input, ...(group.id === FIRST_GROUP ? {} : { group_id: group.id }), id: randomUUID(), author_id: user.user_id, created_at: Date.now(), likes: [] };
        const author_name = await memberChange(storage, secret_hash, (s, current) => {
          if (s.posts.length >= MAX_POSTS) fail(409, 'Feed is full, delete older posts');
          // Recipients must be members when the share is sent.
          const here = groupById(s, group.id) ?? fail(404, 'Group not found');
          if (post.to?.some(id => !memberOf(here, id))) fail(400, 'Invalid recipients');
          s.posts.push(post);
          return current.display_name;
        }, signal);
        const { likes, group_id: _group, ...result } = post;
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
          if (!post || !inGroup(post) || !visible(post, current.user_id)) return fail(404, 'Post not found');
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
          if (!post || !inGroup(post) || !visible(post, current.user_id)) return fail(404, 'Post not found');
          const reply = post.replies?.find(r => r.id === replies[2]);
          if (!reply) return fail(404, 'Reply not found');
          if (reply.author_id !== current.user_id && !manages(memberOf(groupById(s, group.id) ?? group, current.user_id))) return fail(403, 'You can only delete your own reply');
          post.replies = post.replies!.filter(r => r.id !== reply.id);
        }, signal);
        return json({ ok: true });
      }
      const match = /^\/posts\/([a-f0-9-]{36})(\/like)?$/.exec(path);
      if (match && (method === 'DELETE' || match[2] && method === 'PUT')) {
        await memberChange(storage, secret_hash, s => {
          const post = s.posts.find(p => p.id === match[1]);
          if (!post || !inGroup(post) || !visible(post, user.user_id)) return fail(404, 'Post not found');
          if (match[2]) {
            post.likes = post.likes.filter(id => id !== user.user_id);
            if (method === 'PUT') post.likes.push(user.user_id);
          } else {
            if (post.author_id !== user.user_id && !manages(memberOf(groupById(s, group.id) ?? group, user.user_id))) return fail(403, 'You can only delete your own post');
            s.posts = s.posts.filter(p => p.id !== post.id);
          }
        }, signal);
        return json({ ok: true });
      }
      if (path === '/playlists' && method === 'GET') {
        const names = new Map(state.users.map(u => [u.user_id, u.display_name]));
        return json({ playlists: (state.playlists ?? []).filter(p => p.group_id === group.id && seenByReader(p, user.user_id))
          .sort((a, b) => b.created_at - a.created_at)
          .map(p => ({ id: p.id, name: p.name, by: p.by, by_name: names.get(p.by) ?? 'Former member', created_at: p.created_at, count: p.count, artwork_url: p.artwork_url, to: p.to ?? [] })) });
      }
      if (path === '/playlists' && method === 'POST') {
        if (!seat.rights.share) return fail(403, 'An admin turned off sharing for you in this group');
        const input = await body(request, signal, 768 * 1024);
        const name = text(input.name, 'name', 100);
        if (!Array.isArray(input.items) || !input.items.length || input.items.length > MAX_PLAYLIST_SONGS) return fail(400, 'Invalid items');
        const items = input.items.map(song);
        const to = recipients(input.to);
        const id = randomUUID();
        // The songs first: a list that fails to save must not leave a row pointing at nothing.
        await storage.livePut(playlistKey(id), JSON.stringify(items), PLAYLIST_SECONDS, signal);
        const row: SharedPlaylist = { id, group_id: group.id, name, by: user.user_id, created_at: clock(), count: items.length, artwork_url: items.find(i => i.artwork_url)?.artwork_url ?? null, ...(to.length ? { to } : {}) };
        await memberChange(storage, secret_hash, s => {
          const here = groupById(s, group.id) ?? fail(404, 'Group not found');
          if (to.some(other => !memberOf(here, other))) fail(400, 'Invalid recipients');
          const rows = s.playlists ??= [];
          if (rows.length >= MAX_PLAYLISTS) fail(409, 'Too many shared playlists, delete older ones');
          rows.push(row);
        }, signal);
        return json({ id, name, by: user.user_id, by_name: user.display_name, created_at: row.created_at, count: row.count, artwork_url: row.artwork_url, to }, 201);
      }
      const sharedList = /^\/playlists\/([a-f0-9-]{36})$/.exec(path);
      if (sharedList && method === 'GET') {
        const row = (state.playlists ?? []).find(p => p.id === sharedList[1] && p.group_id === group.id);
        if (!row || !seenByReader(row, user.user_id)) return fail(404, 'Playlist not found');
        const [stored] = await storage.liveRead([playlistKey(row.id)], signal);
        if (stored === null) return fail(404, 'Playlist not found');
        // Being opened keeps a list from expiring.
        await storage.livePut(playlistKey(row.id), stored, PLAYLIST_SECONDS, signal);
        return json({ id: row.id, name: row.name, items: JSON.parse(stored) });
      }
      if (sharedList && method === 'DELETE') {
        await memberChange(storage, secret_hash, (s, current) => {
          const row = (s.playlists ?? []).find(p => p.id === sharedList[1] && p.group_id === group.id);
          if (!row) return fail(404, 'Playlist not found');
          if (row.by !== current.user_id && !manages(memberOf(groupById(s, group.id) ?? group, current.user_id))) return fail(403, 'You can only delete your own playlist');
          s.playlists = s.playlists!.filter(p => p !== row);
        }, signal);
        await storage.liveDelete(playlistKey(sharedList[1]), signal);
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

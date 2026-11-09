// Group and audio edge cases: ownership handover and rejoin, what a merge
// carries, the reserved space an unfinished upload holds, and the two ways a
// shared file can be uploaded twice at once.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createService, type VerifySpotify } from '../src/service.js';
import { MAX_USERS, MemoryStorage, type State } from '../src/storage.js';
import { MAX_SONG_BYTES, MONTH_BYTES, STORE_BYTES, type AudioStore } from '../src/audio.js';
import { FIRST_GROUP, MAX_PENDING } from '../src/groups.js';
import { mintEdgeToken } from '../src/edge.js';

const INVITE = 'edge-case-test-invite';
const upload = { title: 'Midnight', artists: ['Ana'], duration_ms: 180_000, size: 8_000_000, ext: 'mp3' };

function fakeStore() {
  const files = new Map<string, number>();
  const removed: string[] = [];
  const tokens: { pathname: string; bytes: number }[] = [];
  const store: AudioStore = {
    async uploadToken(pathname, bytes) { tokens.push({ pathname, bytes }); return `token-${pathname}`; },
    async find(pathname) {
      const size = files.get(pathname);
      return size === undefined ? null : { url: `https://blob/${pathname}`, size };
    },
    async remove(pathnames) { removed.push(...pathnames); },
  };
  return { store, files, removed, tokens };
}

function world() {
  let now = 1_000_000;
  const verify: VerifySpotify = async (token) => ({ id: token, display_name: token.toUpperCase() });
  const storage = new MemoryStorage(() => now);
  const fake = fakeStore();
  const service = createService(storage, INVITE, undefined, () => now, verify, '', fake.store);
  const call = async (secret: string | null, method: string, path: string, data?: unknown, group?: string) => {
    const response = await service(new Request(`http://localhost/api${path}`, {
      method,
      headers: {
        ...(secret ? { Authorization: `Bearer ${secret}` } : {}),
        ...(data === undefined ? {} : { 'Content-Type': 'application/json' }),
        ...(group ? { 'X-Siphon-Group': group } : {}),
      },
      ...(data === undefined ? {} : { body: JSON.stringify(data) }),
    }));
    return { status: response.status, body: await response.json() as any };
  };
  const signIn = async (token: string, device = 'Desk', held: string | null = null) => {
    const { status, body } = await call(held, 'POST', '/auth/spotify', { token, device });
    assert.equal(status, 201, JSON.stringify(body));
    return { id: body.user_id as string, secret: body.secret as string, name: body.display_name as string };
  };
  const legacy = async (display_name: string) => {
    const { body } = await call(null, 'POST', '/join', { invite_code: INVITE, display_name });
    return { id: body.user_id as string, secret: body.secret as string };
  };
  return { call, signIn, legacy, storage, fake, now: () => now };
}

const uuid = (n: number) => `00000000-0000-4000-8000-${n.toString(16).padStart(12, '0')}`;

async function sharedSong(w: ReturnType<typeof world>, owner: { secret: string }, group: string) {
  const reserved = await w.call(owner.secret, 'POST', '/audio', upload, group);
  assert.equal(reserved.status, 201, JSON.stringify(reserved.body));
  w.fake.files.set(reserved.body.pathname, upload.size);
  const completed = await w.call(owner.secret, 'POST', `/audio/${reserved.body.id}/complete`, {}, group);
  assert.equal(completed.status, 200, JSON.stringify(completed.body));
  return reserved.body as { id: string; pathname: string };
}

test('two plays racing at the month boundary cannot spend past the cap', {
  todo: 'service.ts /audio/:id/play checks audio_month bytes on the request snapshot, so two concurrent plays both pass and both add the song',
}, async () => {
  const w = world();
  const ana = await w.signIn('ana');
  const { body: group } = await w.call(ana.secret, 'POST', '/groups', { name: 'G' });
  const ben = await w.signIn('ben');
  await w.call(ben.secret, 'POST', '/groups/join', { code: group.invite });
  const song = await sharedSong(w, ana, group.id);
  const state = await w.storage.read();
  state.audio_month = { month: new Date(w.now()).toISOString().slice(0, 7), bytes: MONTH_BYTES - upload.size };
  state.revision++;
  assert(await w.storage.compareAndSet(state.revision - 1, state));
  const plays = await Promise.all([
    w.call(ben.secret, 'POST', `/audio/${song.id}/play`, {}, group.id),
    w.call(ben.secret, 'POST', `/audio/${song.id}/play`, {}, group.id),
  ]);
  assert.deepEqual(plays.map((play) => play.status), [200, 200], 'both fits were decided before either write');
  const after = await w.storage.read();
  assert.ok(after.audio_month!.bytes <= MONTH_BYTES, `spent ${after.audio_month!.bytes} of ${MONTH_BYTES}`);
});

test('space held by unfinished uploads is never overfilled by a new one', {
  todo: 'audio.ts makeRoom() only evicts finished songs, so reservations alone can push the store past STORE_BYTES',
}, async () => {
  const w = world();
  const ana = await w.signIn('ana');
  const { body: group } = await w.call(ana.secret, 'POST', '/groups', { name: 'G' });
  const state = await w.storage.read();
  state.audio = Array.from({ length: 23 }, (_, index) => ({
    id: uuid(1000 + index), group_id: group.id, by: ana.id, title: `held-${index}`, artists: [],
    duration_ms: 1000, size: MAX_SONG_BYTES, ext: 'mp3', created_at: 1, url: null,
  }));
  state.revision++;
  assert(await w.storage.compareAndSet(state.revision - 1, state));
  const response = await w.call(ana.secret, 'POST', '/audio', { ...upload, title: 'One more' }, group.id);
  assert.equal(response.status, 201);
  const after = await w.storage.read();
  const used = after.audio!.reduce((sum, row) => sum + row.size, 0);
  assert.ok(used <= STORE_BYTES, `used ${used} of ${STORE_BYTES}`);
});

test('two reservations of the same file at once still leave one row', {
  todo: 'audio.ts duplicate check runs on the request snapshot, so two simultaneous POST /audio calls both reserve the same title and size',
}, async () => {
  const w = world();
  const ana = await w.signIn('ana');
  const { body: group } = await w.call(ana.secret, 'POST', '/groups', { name: 'G' });
  const responses = await Promise.all([
    w.call(ana.secret, 'POST', '/audio', upload, group.id),
    w.call(ana.secret, 'POST', '/audio', upload, group.id),
  ]);
  assert.deepEqual(responses.map((response) => response.status), [201, 201]);
  const after = await w.storage.read();
  assert.equal(after.audio!.length, 1, 'the same file is reserved once');
});

test('a merged account keeps the songs it reserved and shared before merging', {
  todo: 'groups.ts mergeUsers() moves posts, likes, playlists and seats but not SharedAudio.by, so the merged account cannot finish or delete its own upload',
}, async () => {
  const w = world();
  const account = await w.signIn('ana', 'Laptop');
  const old = await w.legacy('Ana');
  const reserved = await w.call(old.secret, 'POST', '/audio', upload);
  assert.equal(reserved.status, 201, JSON.stringify(reserved.body));
  const merged = await w.signIn('ana', 'Desk', old.secret);
  assert.equal(merged.id, account.id, 'the older identity folds into the Spotify account');
  w.fake.files.set(reserved.body.pathname, upload.size);
  const complete = await w.call(merged.secret, 'POST', `/audio/${reserved.body.id}/complete`, {});
  assert.equal(complete.status, 200, 'the merged account is the uploader');
  const removed = await w.call(merged.secret, 'DELETE', `/audio/${reserved.body.id}`);
  assert.equal(removed.status, 200);
});

test('a member of a full group can rejoin it with its invite', {
  todo: 'groups.ts /groups/join refuses on the member count before noticing the caller already has a seat, so re-sending the code answers 409',
}, async () => {
  const w = world();
  const me = await w.legacy('Me');
  const state = await w.storage.read();
  const group = state.groups!.find((candidate) => candidate.id === FIRST_GROUP)!;
  for (let index = 0; index < MAX_USERS - 1; index += 1) {
    const id = uuid(2000 + index);
    state.users.push({ user_id: id, display_name: `P${index}`, secret_hash: (index + 1).toString(16).padStart(64, '0') });
    group.members.push({ user_id: id, role: 'member', rights: { share: true, host: true, queue: true, invite: false } });
  }
  state.revision++;
  assert(await w.storage.compareAndSet(state.revision - 1, state));
  const again = await w.call(me.secret, 'POST', '/groups/join', { code: INVITE });
  assert.equal(again.status, 200, JSON.stringify(again.body));
  assert.equal(again.body.members.length, MAX_USERS);
});

test('deleting a group takes its shared audio rows with it', {
  todo: 'groups.ts deleteGroup() drops posts and playlists but leaves state.audio rows (and their blob files) behind',
}, async () => {
  const w = world();
  const ana = await w.signIn('ana');
  const { body: group } = await w.call(ana.secret, 'POST', '/groups', { name: 'G' });
  await sharedSong(w, ana, group.id);
  assert.equal((await w.call(ana.secret, 'DELETE', `/groups/${group.id}`)).status, 200);
  const after = await w.storage.read();
  assert.equal(after.audio!.length, 0, 'no rows point at a group that no longer exists');
});

test('the last member leaving a group deletes it, and its old invite stops working', async () => {
  const w = world();
  const owner = await w.signIn('owner');
  const { body: group } = await w.call(owner.secret, 'POST', '/groups', { name: 'Gone' });
  const code = group.invite as string;
  assert.equal((await w.call(owner.secret, 'POST', `/groups/${group.id}/leave`)).status, 200);
  assert.deepEqual((await w.call(owner.secret, 'GET', '/account')).body.groups, []);
  const late = await w.signIn('late');
  assert.equal((await w.call(late.secret, 'POST', '/groups/join', { code })).status, 403);
});

test('an owner who leaves a group keeps only a member seat when they come back', async () => {
  const w = world();
  const owner = await w.signIn('owner');
  const { body: group } = await w.call(owner.secret, 'POST', '/groups', { name: 'Studio' });
  const heir = await w.signIn('heir');
  await w.call(heir.secret, 'POST', '/groups/join', { code: group.invite });
  assert.equal((await w.call(owner.secret, 'POST', `/groups/${group.id}/leave`)).status, 200);
  const back = await w.call(owner.secret, 'POST', '/groups/join', { code: group.invite });
  assert.equal(back.status, 200, JSON.stringify(back.body));
  assert.equal(back.body.owner_id, heir.id);
  assert.equal(back.body.members.find((member: any) => member.user_id === owner.id).role, 'member');
});

test('merging an identity into an account carries the groups it owned', async () => {
  const w = world();
  const account = await w.signIn('owner', 'Laptop');
  const old = await w.legacy('Owner');
  const { body: group } = await w.call(old.secret, 'POST', '/groups', { name: 'Mine' });
  const merged = await w.signIn('owner', 'Desk', old.secret);
  assert.equal(merged.id, account.id);
  const { body: accountView } = await w.call(merged.secret, 'GET', '/account');
  const mine = accountView.groups.find((candidate: any) => candidate.id === group.id);
  assert.equal(mine.role, 'owner');
  assert.equal(mine.owner_id, account.id);
});

test('an edge token minted by the group service dies on its expiry second', async () => {
  const path = pathToFileURL(join(import.meta.dirname, '..', '..', 'edge', 'src', 'tokens.mjs')).href;
  const { verifyToken } = await import(path) as {
    verifyToken(token: string, secret: string, options?: { now?: number }): { exp: number };
  };
  const secret = 'edge-test-secret-that-is-long-enough-0123456789';
  const exp = 1_700_000_000;
  const token = mintEdgeToken({ sub: 'u1', role: 'phone', scope: 'live', exp }, secret, exp * 1000 - 5000);
  assert.equal(verifyToken(token, secret, { now: exp * 1000 - 1 }).exp, exp);
  assert.throws(() => verifyToken(token, secret, { now: exp * 1000 }), (error: any) => error.reason === 'expired');
  // A changed payload or signature is not the token that was signed.
  const [prefix, payload, signature] = token.split('.');
  const claims = JSON.parse(Buffer.from(payload, 'base64url').toString('utf8'));
  claims.sub = 'u2';
  const forged = `${prefix}.${Buffer.from(JSON.stringify(claims)).toString('base64url')}.${signature}`;
  assert.throws(() => verifyToken(forged, secret), (error: any) => error.reason === 'bad_signature');
});

test('the waiting list has a ceiling, and taking one back makes room', async () => {
  const w = world();
  const owner = await w.signIn('owner');
  const { body: group } = await w.call(owner.secret, 'POST', '/groups', { name: 'Studio' });
  const state = await w.storage.read();
  const stored = state.groups!.find((candidate) => candidate.id === group.id)!;
  stored.pending = Array.from({ length: MAX_PENDING }, (_, index) => `waiting-${index}`);
  state.revision++;
  assert(await w.storage.compareAndSet(state.revision - 1, state));
  const full = await w.call(owner.secret, 'POST', `/groups/${group.id}/members`, { spotify_id: 'one-more' });
  assert.equal(full.status, 409, JSON.stringify(full.body));
  assert.equal((await w.call(owner.secret, 'DELETE', `/groups/${group.id}/members`, { spotify_id: 'waiting-0' })).status, 200);
  const room = await w.call(owner.secret, 'POST', `/groups/${group.id}/members`, { spotify_id: 'one-more' });
  assert.equal(room.status, 201);
  assert(room.body.pending.includes('one-more'));
  assert.equal(room.body.pending.includes('waiting-0'), false);
});

test('a removed member is let back in by an admin, and only then can rejoin', async () => {
  const w = world();
  const owner = await w.signIn('owner');
  const { body: group } = await w.call(owner.secret, 'POST', '/groups', { name: 'Studio' });
  const guest = await w.signIn('guest');
  assert.equal((await w.call(guest.secret, 'POST', '/groups/join', { code: group.invite })).status, 200);
  assert.equal((await w.call(owner.secret, 'DELETE', `/groups/${group.id}/members/${guest.id}`)).status, 200);
  assert.equal((await w.call(guest.secret, 'POST', '/groups/join', { code: group.invite })).status, 403);
  assert.equal((await w.call(owner.secret, 'POST', `/groups/${group.id}/members`, { spotify_id: 'guest' })).status, 201);
  const back = await w.call(guest.secret, 'POST', '/groups/join', { code: group.invite });
  assert.equal(back.status, 200, JSON.stringify(back.body));
  assert.equal((await w.call(guest.secret, 'GET', '/posts', undefined, group.id)).status, 200);
});

test('an invite code opens its own group only', async () => {
  const w = world();
  const one = await w.signIn('one');
  const two = await w.signIn('two');
  const { body: first } = await w.call(one.secret, 'POST', '/groups', { name: 'First' });
  const { body: second } = await w.call(two.secret, 'POST', '/groups', { name: 'Second' });
  const guest = await w.signIn('guest');
  assert.equal((await w.call(guest.secret, 'POST', '/groups/join', { code: first.invite })).status, 200);
  assert.equal((await w.call(guest.secret, 'POST', '/groups/join', { code: second.invite })).status, 200);
  const outsider = await w.signIn('outsider');
  assert.equal((await w.call(outsider.secret, 'POST', '/groups/join', { code: 'made-up-code' })).status, 403);
});

test('only the owner hands the group over, and only to a member', async () => {
  const w = world();
  const owner = await w.signIn('owner');
  const { body: group } = await w.call(owner.secret, 'POST', '/groups', { name: 'Studio' });
  const heir = await w.signIn('heir');
  await w.call(heir.secret, 'POST', '/groups/join', { code: group.invite });
  await w.call(owner.secret, 'PATCH', `/groups/${group.id}/members/${heir.id}`, { role: 'admin' });
  assert.equal((await w.call(heir.secret, 'POST', `/groups/${group.id}/owner`, { user_id: heir.id })).status, 403);
  assert.equal((await w.call(owner.secret, 'POST', `/groups/${group.id}/owner`, { user_id: uuid(9999) })).status, 404);
  const handed = await w.call(owner.secret, 'POST', `/groups/${group.id}/owner`, { user_id: heir.id });
  assert.equal(handed.status, 200);
  assert.equal(handed.body.owner_id, heir.id);
  assert.equal(handed.body.members.find((member: any) => member.user_id === owner.id).role, 'admin');
  assert.equal(handed.body.members.find((member: any) => member.user_id === owner.id).rights.invite, true);
});

test('a fresh group document still has room for its first shared song', async () => {
  const w = world();
  const ana = await w.signIn('ana');
  const { body: group } = await w.call(ana.secret, 'POST', '/groups', { name: 'G' });
  const song = await sharedSong(w, ana, group.id);
  const listed = await w.call(ana.secret, 'GET', '/audio', undefined, group.id);
  assert.equal(listed.body.songs.length, 1);
  assert.equal(listed.body.songs[0].id, song.id);
  assert.equal(listed.body.used, upload.size);
  assert.equal(w.fake.tokens.length, 1);
  const state: State = await w.storage.read();
  assert.equal(state.audio!.length, 1);
});

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createService, hash, validateUrl } from '../src/service.ts';
import { MemoryStorage, RedisStorage, configuredStorage } from '../src/storage.ts';

const invite = 'a-long-test-invite';
const post = { url: 'https://soundcloud.com/friend/song/s-private?secret_token=s-private', source: 'soundcloud', title: 'Beat', artists: ['Friend'], artwork_url: null, duration_ms: 12345, note: 'Listen to this' };
function fixture() {
  const store = new MemoryStorage();
  const service = createService(store, invite);
  const call = async (path: string, method = 'GET', input?: unknown, secret?: string, ip = 'local') => {
    const response = await service(new Request(`https://test/api${path}`, {
      method, headers: { 'content-type': 'application/json', ...(secret ? { authorization: `Bearer ${secret}` } : {}) },
      body: input === undefined ? undefined : JSON.stringify(input),
    }), ip);
    return { status: response.status, body: await response.json() as any, response };
  };
  const join = async (name: string) => (await call('/join', 'POST', { invite_code: invite, display_name: name })).body as { user_id: string; secret: string };
  return { store, call, join, service };
}

test('closed group lifecycle, rename, private link, hearts, ownership and paging', async () => {
  const { store, call, join } = fixture();
  assert.equal((await call('/posts')).status, 401);
  assert.equal((await call('/join', 'POST', { invite_code: 'wrong', display_name: 'A' })).status, 403);
  const a = await join('Alice');
  const b = await join('Bob');
  assert.notEqual(a.user_id, b.user_id);
  const state = await store.read();
  assert.equal(state.users[0].secret_hash, hash(a.secret));
  assert(!JSON.stringify(state).includes(a.secret));
  const first = await call('/posts', 'POST', post, a.secret);
  assert.equal(first.status, 201);
  assert.equal(first.body.url, post.url);
  const second = await call('/posts', 'POST', { ...post, title: 'Second' }, b.secret);
  const feed = await call('/posts?limit=1', 'GET', undefined, a.secret);
  assert.equal(feed.body.posts.length, 1);
  assert(feed.body.next_cursor);
  const next = await call(`/posts?limit=1&cursor=${feed.body.next_cursor}`, 'GET', undefined, a.secret);
  assert.notEqual(next.body.posts[0].id, feed.body.posts[0].id);
  assert.equal(next.body.next_cursor, null);
  assert.equal((await call(`/posts/${first.body.id}`, 'DELETE', undefined, b.secret)).status, 403);
  await call(`/posts/${first.body.id}/like`, 'PUT', undefined, b.secret);
  await call(`/posts/${first.body.id}/like`, 'PUT', undefined, b.secret);
  let page = (await call('/posts', 'GET', undefined, b.secret)).body.posts;
  assert.equal(page.find((p: any) => p.id === first.body.id).like_count, 1);
  assert.equal(page.find((p: any) => p.id === first.body.id).liked, true);
  await call('/me', 'PATCH', { display_name: 'Alicia' }, a.secret);
  page = (await call('/posts', 'GET', undefined, b.secret)).body.posts;
  assert.equal(page.find((p: any) => p.id === first.body.id).author_name, 'Alicia');
  assert(!JSON.stringify(page).includes('secret_hash'));
  await call(`/posts/${first.body.id}/like`, 'DELETE', undefined, b.secret);
  await call(`/posts/${first.body.id}`, 'DELETE', undefined, a.secret);
  assert.equal((await call('/posts', 'GET', undefined, a.secret)).body.posts[0].id, second.body.id);
});

test('allowlist keeps private links and rejects profiles, injection and foreign hosts', () => {
  for (const [source, url] of [
    ['spotify', 'https://open.spotify.com/intl-nl/album/ABC123'],
    ['spotify', 'https://open.spotify.com/playlist/ABC123'],
    ['soundcloud', post.url],
    ['soundcloud', 'https://m.soundcloud.com/a/sets/b/s-secret'],
    ['soundcloud', 'https://on.soundcloud.com/Abc'],
    ['youtube', 'https://youtube.com/watch?v=abcdefghijk'],
    ['youtube', 'https://music.youtube.com/watch?v=abcdefghijk'],
    ['youtube', 'https://youtu.be/abcdefghijk?t=4'],
  ]) assert.equal(validateUrl(url, source), url);
  for (const [source, url] of [
    ['spotify', 'https://open.spotify.com/artist/ABC'],
    ['spotify', 'https://open.spotify.com.evil/track/ABC'],
    ['spotify', 'https://u:p@open.spotify.com/track/ABC'],
    ['spotify', 'http://open.spotify.com/track/ABC'],
    ['soundcloud', 'https://soundcloud.com/a'],
    ['soundcloud', 'https://soundcloud.com/a/likes'],
    ['soundcloud', 'https://soundcloud.com/a/sets'],
    ['soundcloud', 'https://soundcloud.com:8080/a/b'],
    ['youtube', 'https://youtube.com/watch?v=bad'],
    ['youtube', 'https://youtu.be/abcdefghijk/evil'],
    ['youtube', 'https://youtube.com/channel/abcdefghijk'],
    ['spotify', 'https://open.spotify.com/track/ABC%0a'],
  ]) assert.throws(() => validateUrl(url, source), url);
});

test('validation, body bounds, bad cursor and shared join rate limit', async () => {
  const { call, join, service } = fixture();
  const a = await join('A');
  for (const input of [{ ...post, note: 'x'.repeat(281) }, { ...post, duration_ms: -1 }, { ...post, title: '' }, { ...post, artists: 'bad' }, { ...post, artwork_url: 'javascript:bad' }]) {
    assert.equal((await call('/posts', 'POST', input, a.secret)).status, 400);
  }
  assert.equal((await call('/posts?cursor=bad', 'GET', undefined, a.secret)).status, 400);
  assert.equal((await call('/posts?limit=101', 'GET', undefined, a.secret)).status, 400);
  assert.equal((await call('/posts', 'POST', { ...post, note: 'x'.repeat(20000) }, a.secret)).status, 413);
  assert.equal((await service(new Request('http://test/api/join', { method: 'POST', headers: { 'content-type': 'application/json' }, body: '[' }))).status, 400);
  for (let i = 0; i < 10; i++) await call('/join', 'POST', { invite_code: 'wrong', display_name: 'A' }, undefined, 'other');
  const limited = await call('/join', 'POST', { invite_code: invite, display_name: 'A' }, undefined, 'other');
  assert.equal(limited.status, 429);
  assert.equal(limited.response.headers.get('retry-after'), '60');
});

test('concurrent updates retain every post and distinct heart', async () => {
  const { call, join } = fixture();
  const a = await join('A');
  const b = await join('B');
  const created = await Promise.all(Array.from({ length: 8 }, (_, n) => call('/posts', 'POST', { ...post, title: String(n) }, a.secret)));
  assert(created.every(p => p.status === 201));
  const id = created[0].body.id;
  await Promise.all([a, b].map(u => call(`/posts/${id}/like`, 'PUT', undefined, u.secret)));
  const feed = await call('/posts', 'GET', undefined, a.secret);
  assert.equal(feed.body.posts.length, 8);
  assert.equal(feed.body.posts.find((p: any) => p.id === id).like_count, 2);
});

test('Redis REST adapter uses atomic CAS and expiring counters and fails closed', async () => {
  const original = globalThis.fetch;
  const commands: any[][] = [];
  globalThis.fetch = (async (_url: unknown, init: RequestInit) => {
    assert.equal(new Headers(init.headers).get('authorization'), 'Bearer test-token');
    const command = JSON.parse(String(init.body));
    commands.push(command);
    return new Response(JSON.stringify({ result: command[0] === 'GET' ? null : 1 }));
  }) as typeof fetch;
  try {
    const store = new RedisStorage('https://redis.invalid', 'test-token');
    const state = await store.read();
    assert.equal(state.revision, 0);
    assert(await store.compareAndSet(0, { ...state, revision: 1 }));
    assert(await store.limit('join:hash', 2, 3600));
    assert.equal(commands[1][0], 'EVAL');
    assert(commands[1][1].includes('revision'));
    assert(commands[2][1].includes('EXPIRE'));
    assert.throws(() => configuredStorage({ VERCEL: '1' }));
    assert(configuredStorage({ UPSTASH_REDIS_REST_URL: 'https://redis.invalid', UPSTASH_REDIS_REST_TOKEN: 'token' }) instanceof RedisStorage);
  } finally { globalThis.fetch = original; }
});

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createService, hash, validateUrl } from '../src/service.ts';
import { MemoryStorage, RedisStorage, configuredStorage, type Storage } from '../src/storage.ts';

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
  assert.equal(limited.response.headers.get('retry-after'), '3600');
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

test('Redis configuration never combines credentials from different naming schemes', () => {
  for (const env of [
    { KV_REST_API_URL: 'https://first.invalid', UPSTASH_REDIS_REST_TOKEN: 'second' },
    { KV_REST_API_TOKEN: 'first', UPSTASH_REDIS_REST_URL: 'https://second.invalid' },
    { KV_REST_API_URL: 'https://first.invalid', UPSTASH_REDIS_REST_URL: 'https://second.invalid', UPSTASH_REDIS_REST_TOKEN: 'second' },
  ]) assert.throws(() => configuredStorage(env));
  for (const url of ['http://redis.invalid', 'https://user:password@redis.invalid', 'https://redis.invalid?token=secret']) {
    assert.throws(() => new RedisStorage(url, 'token'));
  }
});

test('Redis rejects missing, malformed and corrupt replies without replacing stored data', async () => {
  const original = globalThis.fetch;
  let reply: unknown;
  globalThis.fetch = (async () => new Response(JSON.stringify(reply))) as typeof fetch;
  try {
    const store = new RedisStorage('https://redis.invalid', 'token');
    for (const value of [null, {}, { error: '' }, { result: false }, { result: '{}' }, { result: '{"revision":0,"users":{},"posts":[]}' }]) {
      reply = value;
      await assert.rejects(store.read());
    }
    for (const result of [null, '1', -1, false]) {
      reply = { result };
      await assert.rejects(store.limit('test', 1, 60));
      await assert.rejects(store.compareAndSet(0, { revision: 1, users: [], posts: [] }));
    }
    reply = { result: 0 };
    assert.equal(await store.compareAndSet(0, { revision: 1, users: [], posts: [] }), false);
    await assert.rejects(store.limit('test', 1, 60));
  } finally { globalThis.fetch = original; }
});

test('separate Redis-backed service instances retain posts and resolve CAS conflicts', async () => {
  const original = globalThis.fetch;
  let persisted: string | null = null;
  const counters = new Map<string, number>();
  let conflicts = 0;
  globalThis.fetch = (async (_url: unknown, init: RequestInit) => {
    assert.equal(init.redirect, 'error');
    assert(init.signal);
    const command = JSON.parse(String(init.body));
    let result: unknown;
    if (command[0] === 'GET') result = persisted;
    else if (command[1].includes("redis.call('INCR'")) {
      const key = command[3];
      result = (counters.get(key) ?? 0) + 1;
      counters.set(key, result as number);
    } else {
      const revision = persisted ? JSON.parse(persisted).revision : 0;
      if (revision === command[4]) { persisted = command[5]; result = 1; }
      else { conflicts++; result = 0; }
    }
    return new Response(JSON.stringify({ result }));
  }) as typeof fetch;
  try {
    const make = () => createService(new RedisStorage('https://redis.invalid', 'token'), invite);
    const first = make();
    const request = (path: string, method: string, data?: unknown, secret?: string) => new Request(`https://test/api${path}`, {
      method, headers: { 'content-type': 'application/json', ...(secret ? { authorization: `Bearer ${secret}` } : {}) },
      body: data === undefined ? undefined : JSON.stringify(data),
    });
    const joined = await first(request('/join', 'POST', { invite_code: invite, display_name: 'A' }));
    const { secret } = await joined.json() as { secret: string };
    const second = make();
    const writes = await Promise.all([first, second].map(service => service(request('/posts', 'POST', post, secret))));
    assert(writes.every(response => response.status === 201));
    assert(conflicts > 0);
    const feed = await make()(request('/posts', 'GET', undefined, secret));
    const data = await feed.json() as { posts: { url: string }[] };
    assert.equal(data.posts.length, 2);
    assert(data.posts.every(p => p.url === post.url));
    assert(!persisted!.includes(secret));
  } finally { globalThis.fetch = original; }
});

test('read and write budgets are independent and invalid configuration fails closed', async () => {
  const { call, join } = fixture();
  const a = await join('A');
  for (let i = 0; i < 60; i++) assert.equal((await call('/me', 'GET', undefined, a.secret)).status, 200);
  for (let i = 0; i < 60; i++) assert.equal((await call('/me', 'PATCH', { display_name: 'B' }, a.secret)).status, 200);
  assert.equal((await call('/me', 'PATCH', { display_name: 'C' }, a.secret)).status, 429);
  for (const code of ['too short', 'x'.repeat(257), 'space around invite ', 'control\ncharacters']) {
    const service = createService(new MemoryStorage(), code);
    assert.equal((await service(new Request('https://test/api/me'))).status, 503);
  }
});

test('leaving revokes the bearer, removes hearts and retains shared posts without the old name', async () => {
  const { call, join, store } = fixture();
  const a = await join('Alice');
  const b = await join('Bob');
  const created = await call('/posts', 'POST', post, a.secret);
  const path = `/posts/${created.body.id}/like`;
  await call(path, 'PUT', undefined, a.secret);
  await call(path, 'PUT', undefined, b.secret);
  assert.equal((await call('/me', 'DELETE', undefined, a.secret)).status, 200);
  assert.equal((await call('/me', 'GET', undefined, a.secret)).status, 401);
  assert.equal((await call('/posts', 'POST', post, a.secret)).status, 401);
  const remaining = (await call('/posts', 'GET', undefined, b.secret)).body.posts;
  assert.equal(remaining.length, 1);
  assert.equal(remaining[0].url, post.url);
  assert.equal(remaining[0].author_name, 'Former member');
  assert.equal(remaining[0].like_count, 1);
  assert.equal((await store.read()).users.some(u => u.user_id === a.user_id), false);
});

test('requests authenticated before leave cannot commit a mutation after revocation', async () => {
  for (const action of ['post', 'rename', 'like', 'delete']) {
    const memory = new MemoryStorage();
    let block = false;
    let entered!: () => void;
    let release!: () => void;
    const started = new Promise<void>(resolve => { entered = resolve; });
    const resume = new Promise<void>(resolve => { release = resolve; });
    const storage: Storage = {
      read: () => memory.read(),
      async compareAndSet(previous, next) {
        if (block) { block = false; entered(); await resume; }
        return memory.compareAndSet(previous, next);
      },
      limit: (key, max, seconds) => memory.limit(key, max, seconds),
    };
    const service = createService(storage, invite);
    const request = (path: string, method: string, data?: unknown, secret?: string) => service(new Request(`https://test/api${path}`, {
      method, headers: { 'content-type': 'application/json', ...(secret ? { authorization: `Bearer ${secret}` } : {}) },
      body: data === undefined ? undefined : JSON.stringify(data),
    }));
    const joined = await request('/join', 'POST', { invite_code: invite, display_name: 'A' });
    const { secret } = await joined.json() as { secret: string };
    const created = await request('/posts', 'POST', post, secret);
    const { id } = await created.json() as { id: string };
    block = true;
    const pending = action === 'post' ? request('/posts', 'POST', post, secret) :
      action === 'rename' ? request('/me', 'PATCH', { display_name: 'Too late' }, secret) :
      action === 'like' ? request(`/posts/${id}/like`, 'PUT', undefined, secret) :
      request(`/posts/${id}`, 'DELETE', undefined, secret);
    await started;
    assert.equal((await request('/me', 'DELETE', undefined, secret)).status, 200);
    release();
    assert.equal((await pending).status, 401, action);
    const state = await memory.read();
    assert.equal(state.posts.length, 1);
    assert.equal(state.posts[0].likes.length, 0);
    assert.equal(state.users.length, 0);
  }
});

test('direct shares reach only their recipients and the author', async () => {
  const { call, join } = fixture();
  const a = await join('Alice');
  const b = await join('Bob');
  const c = await join('Cleo');
  const members = await call('/members', 'GET', undefined, c.secret);
  assert.deepEqual(members.body.members.map((m: any) => m.display_name), ['Alice', 'Bob', 'Cleo']);
  assert(!JSON.stringify(members.body).includes('secret'));
  const sent = await call('/posts', 'POST', { ...post, to: [b.user_id] }, a.secret);
  assert.equal(sent.status, 201);
  assert.deepEqual(sent.body.to, [b.user_id]);
  const titles = async (secret: string) => (await call('/posts', 'GET', undefined, secret)).body.posts.map((p: any) => p.id);
  assert.deepEqual(await titles(a.secret), [sent.body.id]);
  assert.deepEqual(await titles(b.secret), [sent.body.id]);
  assert.deepEqual(await titles(c.secret), []);
  assert.equal((await call(`/posts/${sent.body.id}/like`, 'PUT', undefined, c.secret)).status, 404);
  assert.equal((await call(`/posts/${sent.body.id}/replies`, 'POST', { text: 'hi' }, c.secret)).status, 404);
  assert.equal((await call('/posts', 'POST', { ...post, to: ['00000000-0000-0000-0000-000000000000'] }, a.secret)).status, 400);
  assert.equal((await call('/posts', 'POST', { ...post, to: 'everyone' }, a.secret)).status, 400);
  const open = await call('/posts', 'POST', { ...post, to: [] }, a.secret);
  assert.deepEqual(open.body.to, []);
  assert.deepEqual(await titles(c.secret), [open.body.id]);
});

test('replies thread under a share, keep their author and can be removed by them', async () => {
  const { call, join, store } = fixture();
  const a = await join('Alice');
  const b = await join('Bob');
  const shared = await call('/posts', 'POST', post, a.secret);
  const reply = await call(`/posts/${shared.body.id}/replies`, 'POST', { text: 'That snare though' }, b.secret);
  assert.equal(reply.status, 201);
  assert.equal(reply.body.author_name, 'Bob');
  assert.equal((await call(`/posts/${shared.body.id}/replies`, 'POST', { text: '' }, b.secret)).status, 400);
  assert.equal((await call(`/posts/${shared.body.id}/replies`, 'POST', { text: 'x'.repeat(501) }, b.secret)).status, 400);
  let feed = (await call('/posts', 'GET', undefined, a.secret)).body.posts;
  assert.deepEqual(feed[0].replies.map((r: any) => [r.author_name, r.text]), [['Bob', 'That snare though']]);
  assert.equal((await call(`/posts/${shared.body.id}/replies/${reply.body.id}`, 'DELETE', undefined, a.secret)).status, 403);
  await call('/me', 'DELETE', undefined, b.secret);
  feed = (await call('/posts', 'GET', undefined, a.secret)).body.posts;
  assert.equal(feed[0].replies[0].author_name, 'Former member');
  // The stored document still validates after a member with replies left.
  await store.read();
  const own = await call(`/posts/${shared.body.id}/replies`, 'POST', { text: 'mine' }, a.secret);
  assert.equal((await call(`/posts/${shared.body.id}/replies/${own.body.id}`, 'DELETE', undefined, a.secret)).status, 200);
  feed = (await call('/posts', 'GET', undefined, a.secret)).body.posts;
  assert.equal(feed[0].replies.length, 1);
});

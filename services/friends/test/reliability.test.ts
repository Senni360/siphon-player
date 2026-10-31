import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createService, hash } from '../src/service.js';
import { MAX_STATE_BYTES, MemoryStorage, RedisStorage, type Storage, type State, type Post } from '../src/storage.js';

const invite = 'bounded-stress-fixture-invite';
const payload = { url: 'https://soundcloud.com/fixture/private/s-fixture?secret_token=s-fixture', source: 'soundcloud', title: 'Fixture', artists: ['Fixture artist'], artwork_url: null, duration_ms: 123000, note: 'Fixture only' };
const uuid = (n: number) => `00000000-0000-4000-8000-${n.toString(16).padStart(12, '0')}`;
const tick = () => new Promise<void>(resolve => setImmediate(resolve));

function fixture(storage: Storage = new MemoryStorage(), instances = 6) {
  const services = Array.from({ length: instances }, () => createService(storage, invite));
  let requestNumber = 0;
  const call = async (path: string, method = 'GET', data?: unknown, secret?: string, ip = 'fixture', signal?: AbortSignal) => {
    const response = await services[requestNumber++ % services.length](new Request(`https://fixture.invalid/api${path}`, {
      method, signal, headers: { 'content-type': 'application/json', ...(secret ? { authorization: `Bearer ${secret}` } : {}) },
      body: data === undefined ? undefined : JSON.stringify(data),
    }), ip);
    const body = await response.json() as any;
    assert.equal(response.headers.get('cache-control'), 'no-store');
    return { status: response.status, body, response };
  };
  const join = async (index: number) => {
    const result = await call('/join', 'POST', { invite_code: invite, display_name: `Member ${index}` }, undefined, `join-${index}`);
    assert.equal(result.status, 201);
    return result.body as { user_id: string; secret: string };
  };
  return { storage, call, join };
}

test('bounded six-client burst keeps all 48 posts and idempotent hearts without lost writes', { timeout: 15000 }, async t => {
  const memory = new MemoryStorage();
  let attempts = 0;
  let conflicts = 0;
  const storage: Storage = {
    async read() { await tick(); return memory.read(); },
    async compareAndSet(previous, next) {
      await tick();
      attempts++;
      const accepted = await memory.compareAndSet(previous, next);
      if (!accepted) conflicts++;
      return accepted;
    },
    limit: (key, max, seconds) => memory.limit(key, max, seconds),
  };
  const { call, join } = fixture(storage);
  const users = await Promise.all(Array.from({ length: 6 }, (_, i) => join(i)));
  const started = performance.now();
  const creates = await Promise.all(users.flatMap((user, i) => Array.from({ length: 8 }, (_, n) =>
    call('/posts', 'POST', { ...payload, title: `Member ${i} post ${n}` }, user.secret, `member-${i}`))));
  assert(creates.every(result => result.status === 201), `statuses: ${creates.map(result => result.status).join(',')}`);
  assert.equal(new Set(creates.map(result => result.body.id)).size, 48);
  const id = creates[0].body.id;
  const hearts = await Promise.all(users.flatMap((user, i) => Array.from({ length: 3 }, () =>
    call(`/posts/${id}/like`, 'PUT', undefined, user.secret, `member-${i}`))));
  assert(hearts.every(result => result.status === 200));
  const state = await memory.read();
  assert.equal(state.posts.length, 48);
  assert.equal(state.posts.find(post => post.id === id)!.likes.length, 6);
  assert(state.users.every(user => !JSON.stringify(state).includes(users.find(u => u.user_id === user.user_id)!.secret)));
  t.diagnostic(`fixture only: 6 members, 48 creates, 18 hearts; ${attempts} CAS attempts, ${conflicts} conflicts, ${Math.round(performance.now() - started)} ms`);
});

test('pagination across equal timestamps survives concurrent creation, deletion, rename and revoke', async () => {
  const memory = new MemoryStorage();
  const { call, join } = fixture(memory);
  const [a, b] = await Promise.all([join(1), join(2)]);
  const state = await memory.read();
  const initial: Post[] = Array.from({ length: 60 }, (_, i) => ({ ...payload, source: 'soundcloud', id: uuid(i + 1), author_id: i % 2 ? a.user_id : b.user_id, created_at: 1_000, likes: [] }));
  assert(await memory.compareAndSet(state.revision, { ...state, revision: state.revision + 1, posts: initial }));
  const first = await call('/posts?limit=7', 'GET', undefined, b.secret);
  const seen = new Set<string>(first.body.posts.map((post: Post) => post.id));
  const deleted = initial.filter(post => post.author_id === a.user_id && Number.parseInt(post.id.slice(-12), 16) < 20).slice(0, 4);
  const changes = await Promise.all([
    ...deleted.map(post => call(`/posts/${post.id}`, 'DELETE', undefined, a.secret)),
    call('/me', 'PATCH', { display_name: 'Renamed' }, a.secret),
    call('/posts', 'POST', { ...payload, title: 'New head' }, b.secret),
  ]);
  assert(changes.every(result => result.status === 200 || result.status === 201));
  assert.equal((await call('/me', 'DELETE', undefined, a.secret)).status, 200);
  let cursor = first.body.next_cursor;
  for (let page = 0; cursor && page < 20; page++) {
    const result = await call(`/posts?limit=7&cursor=${encodeURIComponent(cursor)}`, 'GET', undefined, b.secret);
    assert.equal(result.status, 200);
    for (const post of result.body.posts) {
      assert(!seen.has(post.id), `duplicate ${post.id}`);
      seen.add(post.id);
      if (post.author_id === a.user_id) assert.equal(post.author_name, 'Former member');
    }
    cursor = result.body.next_cursor;
  }
  assert.equal(cursor, null);
  const expected = initial.filter(post => !deleted.some(removed => removed.id === post.id)).map(post => post.id);
  assert.deepEqual([...seen].sort(), expected.sort());
  assert.equal((await call('/posts', 'GET', undefined, a.secret)).status, 401);
});

test('CAS exhaustion and ambiguous committed failures never retry the external mutation', async () => {
  const memory = new MemoryStorage();
  let mode: 'normal' | 'conflict' | 'commit-then-fail' = 'normal';
  let calls = 0;
  const storage: Storage = {
    read: () => memory.read(),
    async compareAndSet(previous, next) {
      calls++;
      if (mode === 'conflict') return false;
      const accepted = await memory.compareAndSet(previous, next);
      if (accepted && mode === 'commit-then-fail') throw new Error('transport failed with a private token');
      return accepted;
    },
    limit: (key, max, seconds) => memory.limit(key, max, seconds),
  };
  const { call, join } = fixture(storage);
  const a = await join(1);
  mode = 'conflict'; calls = 0;
  const conflicted = await call('/posts', 'POST', payload, a.secret);
  assert.equal(conflicted.status, 503);
  assert.equal(calls, 20);
  assert.equal((await memory.read()).posts.length, 0);
  mode = 'commit-then-fail'; calls = 0;
  const uncertain = await call('/posts', 'POST', payload, a.secret);
  assert.equal(uncertain.status, 503);
  assert(!JSON.stringify(uncertain.body).includes('private token'));
  assert.equal(calls, 1);
  assert.equal((await memory.read()).posts.length, 1);
});

test('stored unknown post fields are never returned to group readers', async () => {
  const memory = new MemoryStorage();
  const { call, join } = fixture(memory);
  const a = await join(1);
  const created = await call('/posts', 'POST', payload, a.secret);
  assert.equal(created.status, 201);
  const state = await memory.read();
  Object.assign(state.posts[0], { secret_hash: 'accidentally stored secret', internal_credentials: { token: 'private-token' } });
  assert(await memory.compareAndSet(state.revision, { ...state, revision: state.revision + 1 }));
  const response = await call('/posts', 'GET', undefined, a.secret);
  assert.equal(response.status, 200);
  assert(!JSON.stringify(response.body).includes('accidentally stored secret'));
  assert(!JSON.stringify(response.body).includes('private-token'));
});

test('concurrent reads, writes and join attempts respect shared atomic limits', async () => {
  const { call, join, storage } = fixture();
  const user = await join(1);
  const reads = await Promise.all(Array.from({ length: 130 }, () => call('/me', 'GET', undefined, user.secret, 'read-ip')));
  assert.equal(reads.filter(result => result.status === 200).length, 120);
  assert.equal(reads.filter(result => result.status === 429).length, 10);
  const writes = await Promise.all(Array.from({ length: 70 }, () => call('/me', 'PATCH', { display_name: '' }, user.secret, 'write-ip')));
  assert.equal(writes.filter(result => result.status === 400).length, 60);
  assert.equal(writes.filter(result => result.status === 429).length, 10);
  const joins = await Promise.all(Array.from({ length: 24 }, () => call('/join', 'POST', { invite_code: 'wrong-invite', display_name: 'No' }, undefined, 'join-ip')));
  assert.equal(joins.filter(result => result.status === 403).length, 10);
  assert.equal(joins.filter(result => result.status === 429).length, 14);
  assert(joins.filter(result => result.status === 429).every(result => result.response.headers.get('retry-after') === '3600'));
  assert.equal((await storage.read()).users.length, 1);
});

test('rate buckets expire without extending their window on rejected attempts', async t => {
  t.mock.timers.enable({ apis: ['Date'], now: 1000 });
  const storage = new MemoryStorage();
  assert(await storage.limit('fixture', 2, 60));
  assert(await storage.limit('fixture', 2, 60));
  t.mock.timers.setTime(60_000);
  assert.equal(await storage.limit('fixture', 2, 60), false);
  t.mock.timers.setTime(61_000);
  assert(await storage.limit('fixture', 2, 60));
});

test('abort before a mutation or during conflict retry prevents subsequent CAS work', async () => {
  for (const mode of ['read', 'conflict']) {
    const memory = new MemoryStorage();
    const controller = new AbortController();
    let active = false;
    let reads = 0;
    let attempts = 0;
    const storage: Storage = {
      async read(signal) {
        const state = await memory.read(signal);
        if (active && ++reads === 2 && mode === 'read') controller.abort();
        return state;
      },
      async compareAndSet(previous, next, signal) {
        if (active) { attempts++; controller.abort(); return false; }
        return memory.compareAndSet(previous, next, signal);
      },
      limit: (key, max, seconds, signal) => memory.limit(key, max, seconds, signal),
    };
    const { call, join } = fixture(storage);
    const user = await join(1);
    active = true;
    assert.equal((await call('/posts', 'POST', payload, user.secret, 'fixture', controller.signal)).status, 503);
    assert.equal(attempts, mode === 'read' ? 0 : 1);
    assert.equal((await memory.read()).posts.length, 0);
  }
});

test('request abort cancels a stalled body and a pending Redis request', { timeout: 3000 }, async () => {
  const controller = new AbortController();
  let cancelled = false;
  const service = createService(new MemoryStorage(), invite);
  const request = new Request('https://fixture.invalid/api/join', {
    method: 'POST', headers: { 'content-type': 'application/json' }, signal: controller.signal,
    body: new ReadableStream({ start(stream) { stream.enqueue(new TextEncoder().encode('{')); }, cancel() { cancelled = true; } }),
    duplex: 'half',
  } as RequestInit);
  const pending = service(request);
  await tick(); controller.abort();
  assert.equal((await pending).status, 503);
  assert(cancelled);

  const original = globalThis.fetch;
  let started!: () => void;
  const called = new Promise<void>(resolve => { started = resolve; });
  let calls = 0;
  globalThis.fetch = (async (_url: unknown, init: RequestInit) => {
    calls++; started();
    return new Promise<Response>((_resolve, reject) => init.signal!.addEventListener('abort', () => reject(new Error('do not echo private transport data')), { once: true }));
  }) as typeof fetch;
  try {
    const abort = new AbortController();
    const result = fixture(new RedisStorage('https://fixture.invalid', 'fixture-only-token')).call('/posts', 'GET', undefined, undefined, 'fixture', abort.signal);
    await called; abort.abort();
    const response = await result;
    assert.equal(response.status, 503);
    assert.deepEqual(response.body, { error: 'Service unavailable' });
    assert.equal(calls, 1);
    let bodyCancelled = false;
    globalThis.fetch = (async () => new Response(new ReadableStream({ cancel() { bodyCancelled = true; } }))) as typeof fetch;
    const abortBody = new AbortController();
    const reading = new RedisStorage('https://fixture.invalid', 'fixture-only-token').read(abortBody.signal);
    await tick(); abortBody.abort();
    await assert.rejects(reading);
    assert(bodyCancelled);
  } finally { globalThis.fetch = original; }
});

test('Redis rejects corrupt row schemas, duplicate identities and unsafe source URLs', async () => {
  const original = globalThis.fetch;
  const state: State = {
    revision: 1,
    users: [{ user_id: uuid(1), display_name: 'Fixture', secret_hash: hash('fixture-secret') }],
    posts: [{ ...payload, source: 'soundcloud', id: uuid(2), author_id: uuid(1), created_at: 1000, likes: [uuid(1)] }],
  };
  let persisted = structuredClone(state);
  globalThis.fetch = (async () => new Response(JSON.stringify({ result: JSON.stringify(persisted) }))) as typeof fetch;
  try {
    const storage = new RedisStorage('https://fixture.invalid', 'fixture-only-token');
    assert.equal((await storage.read()).posts[0].url, payload.url);
    const corrupt: ((state: State) => void)[] = [
      state => { state.revision = Number.MAX_SAFE_INTEGER; },
      state => { state.users.push({ ...state.users[0] }); },
      state => { state.posts.push({ ...state.posts[0] }); },
      state => { state.posts[0].created_at = -1; },
      state => { state.posts[0].duration_ms = -1; },
      state => { state.posts[0].likes.push(uuid(1)); },
      state => { state.posts[0].likes = [uuid(999)]; },
      state => { state.posts[0].artists = [null as unknown as string]; },
      state => { state.posts[0].url = 'javascript:alert(1)'; },
      state => { state.posts[0].source = 'spotify'; },
      state => { state.posts[0].artwork_url = 'file:///private'; },
    ];
    for (const mutate of corrupt) {
      persisted = structuredClone(state); mutate(persisted);
      await assert.rejects(storage.read());
    }
  } finally { globalThis.fetch = original; }
});

test('Redis rejects oversized headers and streams, HTTP errors and malformed JSON', async () => {
  const original = globalThis.fetch;
  let cancelled = false;
  let makeResponse = () => new Response('not JSON');
  globalThis.fetch = (async () => makeResponse()) as typeof fetch;
  try {
    const storage = new RedisStorage('https://fixture.invalid', 'fixture-only-token');
    await assert.rejects(storage.read());
    makeResponse = () => new Response('private error contents', { status: 503 });
    await assert.rejects(storage.read());
    makeResponse = () => new Response('small', { headers: { 'content-length': String(MAX_STATE_BYTES * 7) } });
    await assert.rejects(storage.read());
    const chunk = new Uint8Array(1024 * 1024);
    let chunks = 0;
    makeResponse = () => new Response(new ReadableStream({
      pull(stream) { chunks++; stream.enqueue(chunk); },
      cancel() { cancelled = true; },
    }));
    await assert.rejects(storage.read());
    assert(cancelled);
    assert(chunks <= 50, `unbounded response consumed ${chunks} chunks`);
  } finally { globalThis.fetch = original; }
});

test('document byte capacity rejects an oversized mutation without changing existing posts', async () => {
  const memory = new MemoryStorage();
  const { call, join } = fixture(memory);
  const user = await join(1);
  const state = await memory.read();
  const large = { ...payload, url: `${payload.url}&padding=${'a'.repeat(3900)}`, artwork_url: `https://art.invalid/${'a'.repeat(3900)}` };
  const row: Post = { ...large, source: 'soundcloud', id: uuid(100), author_id: user.user_id, created_at: 1000, likes: [] };
  const overhead = Buffer.byteLength(JSON.stringify(state));
  const count = Math.floor((MAX_STATE_BYTES - overhead) / (Buffer.byteLength(JSON.stringify(row)) + 1));
  state.posts = Array.from({ length: count }, (_, i) => ({ ...row, id: uuid(i + 100) }));
  state.revision++;
  assert(await memory.compareAndSet(state.revision - 1, state));
  const result = await call('/posts', 'POST', large, user.secret);
  assert.equal(result.status, 409);
  const retained = await memory.read();
  assert.equal(retained.revision, state.revision);
  assert.equal(retained.posts.length, count);
});

test('exhausted revision cannot wrap or poison the persisted group', async () => {
  const memory = new MemoryStorage();
  const { call, join } = fixture(memory);
  const user = await join(1);
  const state = await memory.read();
  assert(await memory.compareAndSet(state.revision, { ...state, revision: Number.MAX_SAFE_INTEGER - 1 }));
  assert.equal((await call('/posts', 'POST', payload, user.secret)).status, 503);
  assert.equal((await memory.read()).revision, Number.MAX_SAFE_INTEGER - 1);
  assert.equal((await memory.read()).posts.length, 0);
});

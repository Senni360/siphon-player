// Concurrency and request-body edges in the group service: a write that lost
// its authorization, a retry that must not apply twice, two people joining at
// once, and the exact boundaries of the JSON body reader.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createService, hash } from '../src/service.js';
import { body } from '../src/http.js';
import { MemoryStorage, type State, type Storage } from '../src/storage.js';

const INVITE = 'concurrency-test-invite';
const payload = { url: 'https://open.spotify.com/track/abc', source: 'spotify', title: 'T', artists: [], artwork_url: null, duration_ms: 0 };

const liveOf = (memory: MemoryStorage) => ({
  liveRead: memory.liveRead.bind(memory), livePut: memory.livePut.bind(memory), liveDelete: memory.liveDelete.bind(memory),
  liveCas: memory.liveCas.bind(memory), livePublish: memory.livePublish.bind(memory), liveListen: memory.liveListen.bind(memory),
});

function fixture(storage: Storage, instances = 1) {
  const services = Array.from({ length: instances }, () => createService(storage, INVITE));
  let turn = 0;
  const call = async (path: string, method = 'GET', data?: unknown, secret?: string, ip = 'fixture') => {
    const response = await services[turn++ % services.length](new Request(`https://fixture.invalid/api${path}`, {
      method,
      headers: { 'content-type': 'application/json', ...(secret ? { authorization: `Bearer ${secret}` } : {}) },
      body: data === undefined ? undefined : JSON.stringify(data),
    }), ip);
    return { status: response.status, body: await response.json() as any };
  };
  const join = async (name: string) => {
    const result = await call('/join', 'POST', { invite_code: INVITE, display_name: name });
    assert.equal(result.status, 201);
    return result.body as { user_id: string; secret: string };
  };
  return { call, join };
}

test('a write whose authorization was revoked before its commit does not land', async () => {
  const memory = new MemoryStorage();
  let armed = false;
  let revoked = false;
  let author = '';
  const storage: Storage = {
    ...liveOf(memory),
    read: (signal) => memory.read(signal),
    async compareAndSet(previous, next, signal) {
      if (armed && !revoked) {
        revoked = true;
        armed = false;
        // The membership is taken away after the request was authorized but
        // before its compare-and-set can commit.
        const state = await memory.read();
        state.users = state.users.filter(user => user.user_id !== author);
        for (const group of state.groups ?? []) group.members = group.members.filter(member => member.user_id !== author);
        state.revision++;
        assert(await memory.compareAndSet(state.revision - 1, state));
        return false;
      }
      return memory.compareAndSet(previous, next, signal);
    },
    limit: (key, maximum, seconds, signal) => memory.limit(key, maximum, seconds, signal),
  };
  const { call, join } = fixture(storage);
  const me = await join('Me');
  author = me.user_id;
  armed = true;
  const posted = await call('/posts', 'POST', payload, me.secret);
  assert.equal(posted.status, 401, 'the retry re-authorizes and finds no seat');
  assert.equal(revoked, true);
  const state = await memory.read();
  assert.equal(state.posts.length, 0);
  assert.equal(state.users.some(user => user.user_id === me.user_id), false);
});

test('a compare-and-set retry never applies the change twice', async () => {
  const memory = new MemoryStorage();
  let failNext = false;
  const storage: Storage = {
    ...liveOf(memory),
    read: (signal) => memory.read(signal),
    async compareAndSet(previous, next, signal) {
      if (failNext) { failNext = false; return false; }
      return memory.compareAndSet(previous, next, signal);
    },
    limit: (key, maximum, seconds, signal) => memory.limit(key, maximum, seconds, signal),
  };
  const { call, join } = fixture(storage);
  const me = await join('Me');
  failNext = true;
  const posted = await call('/posts', 'POST', payload, me.secret);
  assert.equal(posted.status, 201);
  const state = await memory.read();
  assert.equal(state.posts.length, 1, 'one post, not one per attempt');
  assert.equal(state.posts[0].id, posted.body.id);
});

test('two people joining one group at the same moment both get a seat', async () => {
  const memory = new MemoryStorage();
  const { call } = fixture(memory, 2);
  const owner = await call('/join', 'POST', { invite_code: INVITE, display_name: 'Owner' });
  const created = await call('/groups', 'POST', { name: 'Race' }, owner.body.secret);
  assert.equal(created.status, 201);
  const code = created.body.invite as string;
  const one = await call('/join', 'POST', { invite_code: INVITE, display_name: 'One' });
  const two = await call('/join', 'POST', { invite_code: INVITE, display_name: 'Two' });
  const joins = await Promise.all([
    call('/groups/join', 'POST', { code }, one.body.secret),
    call('/groups/join', 'POST', { code }, two.body.secret),
  ]);
  assert.deepEqual(joins.map(join => join.status), [200, 200]);
  const state = await memory.read();
  const group = state.groups!.find(candidate => candidate.id === created.body.id)!;
  assert.deepEqual(group.members.map(member => member.user_id).sort(), [owner.body.user_id, one.body.user_id, two.body.user_id].sort());
});

test('the JSON body reader accepts exactly its limit and refuses one byte more', async () => {
  const signal = new AbortController().signal;
  const at = new Request('https://fixture.invalid/api/join', {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: '{"a":"12345678"}',
  });
  assert.equal(Buffer.byteLength('{"a":"12345678"}'), 16);
  assert.deepEqual(await body(at, signal, 16), { a: '12345678' });
  const over = new Request('https://fixture.invalid/api/join', {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: '{"a":"123456789"}',
  });
  await assert.rejects(body(over, signal, 16), (error: any) => error.status === 413);
});

test('the JSON body reader refuses a declared, streamed or misspelt body', async () => {
  const signal = new AbortController().signal;
  const declared = new Request('https://fixture.invalid/api/join', {
    method: 'POST', headers: { 'content-type': 'application/json', 'content-length': '1000' }, body: '{"a":1}',
  });
  await assert.rejects(body(declared, signal, 16), (error: any) => error.status === 413);

  let cancelled = false;
  const streamed = new Request('https://fixture.invalid/api/join', {
    method: 'POST', duplex: 'half', headers: { 'content-type': 'application/json' },
    body: new ReadableStream({ start(stream) { stream.enqueue(new TextEncoder().encode('{"a":"123456789"}')); }, cancel() { cancelled = true; } }),
  } as RequestInit);
  await assert.rejects(body(streamed, signal, 16), (error: any) => error.status === 413);
  assert.equal(cancelled, true, 'the rest of an oversized body is cancelled');

  for (const [headers, text] of [
    [{ 'content-type': 'text/plain' }, '{"a":1}'],
    [{ 'content-type': 'application/json; charset=utf-8' }, '{'],
    [{ 'content-type': 'application/json' }, '[]'],
    [{ 'content-type': 'application/json' }, '"text"'],
    [{ 'content-type': 'application/json' }, ''],
  ] as const) {
    const request = new Request('https://fixture.invalid/api/join', { method: 'POST', headers, body: text });
    await assert.rejects(body(request, signal, 1024), (error: any) => error.status === (headers['content-type'] === 'text/plain' ? 415 : 400), text);
  }
  const charset = new Request('https://fixture.invalid/api/join', {
    method: 'POST', headers: { 'content-type': 'application/json; charset=utf-8' }, body: '{"a":1}',
  });
  assert.deepEqual(await body(charset, signal, 1024), { a: 1 });
});

test('aborting while the body is still arriving cancels the stream', async () => {
  const controller = new AbortController();
  let cancelled = false;
  const request = new Request('https://fixture.invalid/api/join', {
    method: 'POST', duplex: 'half', headers: { 'content-type': 'application/json' }, signal: controller.signal,
    body: new ReadableStream({
      start(stream) { stream.enqueue(new TextEncoder().encode('{"a":')); },
      cancel() { cancelled = true; },
    }),
  } as RequestInit);
  const reading = body(request, controller.signal, 1024);
  await new Promise(resolve => setImmediate(resolve));
  controller.abort();
  await assert.rejects(reading);
  assert.equal(cancelled, true);
});

test('joining twice never produces two accounts with one secret hash', async () => {
  // The storage contract rejects a document whose users share a secret hash;
  // the service must never write one in the first place.
  const memory = new MemoryStorage();
  const { call, join } = fixture(memory);
  const one = await join('One');
  const two = await join('Two');
  assert.notEqual(hash(one.secret), hash(two.secret));
  const state: State = await memory.read();
  assert.equal(new Set(state.users.map(user => user.secret_hash)).size, state.users.length);
  assert.equal((await call('/me', 'GET', undefined, one.secret)).status, 200);
});

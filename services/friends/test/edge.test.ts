import { test } from 'node:test';
import assert from 'node:assert/strict';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createService } from '../src/service.js';
import { MemoryStorage } from '../src/storage.js';
import { EDGE_TOKEN_TTL_SECONDS, EDGE_URL } from '../src/edge.js';

const invite = 'a-long-edge-test-invite';
const SECRET = 'edge-test-secret-that-is-long-enough-0123456789';

/** The edge's own verifier, so a token made here is checked by the side that accepts it. */
async function edgeVerifier() {
  const path = pathToFileURL(join(import.meta.dirname, '..', '..', 'edge', 'src', 'tokens.mjs')).href;
  return await import(path) as {
    verifyToken(token: string, secret: string, options?: { role?: string; scope?: string }): {
      sub: string; role: string; scope: string; exp: number; jti: string;
    };
  };
}

function fixture(edgeSecret = SECRET) {
  const store = new MemoryStorage();
  let now = Date.now();
  const service = createService(store, invite, undefined, () => now, undefined, '', null, edgeSecret);
  const call = async (path: string, method = 'GET', input?: unknown, secret?: string) => {
    const response = await service(new Request(`https://test/api${path}`, {
      method, headers: { 'content-type': 'application/json', ...(secret ? { authorization: `Bearer ${secret}` } : {}) },
      body: input === undefined ? undefined : JSON.stringify(input),
    }));
    return { status: response.status, body: await response.json() as Record<string, unknown> };
  };
  const join = async () => (await call('/join', 'POST', { invite_code: invite, display_name: 'Edge' })).body as { user_id: string; secret: string };
  return { store, call, join };
}

test('an edge token minted here verifies with the edge\'s own verifier', async () => {
  const { verifyToken } = await edgeVerifier();
  const { call, join } = fixture();
  const { user_id, secret } = await join();
  assert.equal((await call('/edge/token', 'POST', { role: 'phone' })).status, 401);

  const answer = await call('/edge/token', 'POST', { role: 'phone' }, secret);
  assert.equal(answer.status, 200);
  assert.equal(answer.body.url, EDGE_URL);
  for (const scope of ['live', 'media', 'sync']) {
    const token = answer.body[scope] as string;
    assert.equal(typeof token, 'string');
    const claims = verifyToken(token, SECRET, { role: 'phone', scope });
    assert.equal(claims.sub, user_id);
    assert.equal(claims.role, 'phone');
    assert.equal(claims.scope, scope);
    assert.match(claims.jti, /^[A-Za-z0-9_-]{8,64}$/);
    assert(claims.exp > Math.floor(Date.now() / 1000));
    assert(claims.exp <= Math.floor(Date.now() / 1000) + EDGE_TOKEN_TTL_SECONDS + 1);
  }
  assert(!JSON.stringify(answer.body).includes(SECRET));
});

test('the computer role is minted, other roles are refused', async () => {
  const { verifyToken } = await edgeVerifier();
  const { call, join } = fixture();
  const { secret, user_id } = await join();

  const answer = await call('/edge/token', 'POST', { role: 'computer' }, secret);
  assert.equal(answer.status, 200);
  assert.equal(verifyToken(answer.body.live as string, SECRET, { role: 'computer', scope: 'live' }).sub, user_id);

  for (const role of ['tablet', '', null, 7]) {
    assert.equal((await call('/edge/token', 'POST', { role }, secret)).status, 400);
  }
});

test('without EDGE_TOKEN_SECRET the route answers 503', async () => {
  const { call, join } = fixture('');
  const { secret } = await join();
  assert.equal((await call('/edge/token', 'POST', { role: 'phone' }, secret)).status, 503);
  // A secret shorter than the edge's minimum is the same as none at all.
  const short = fixture('too-short');
  assert.equal((await short.call('/edge/token', 'POST', { role: 'phone' }, (await short.join()).secret)).status, 503);
});

test('edge tokens are rate limited per account', async () => {
  const { call, join } = fixture();
  const { secret } = await join();
  for (let i = 0; i < 30; i++) assert.equal((await call('/edge/token', 'POST', { role: 'phone' }, secret)).status, 200);
  const limited = await call('/edge/token', 'POST', { role: 'phone' }, secret);
  assert.equal(limited.status, 429);
  assert.equal(limited.body.error, 'Too many requests');
});

test('only people in a group get a way to the edge', async () => {
  const { call, join } = fixture();
  const { secret } = await join();
  assert.equal((await call('/edge/token', 'POST', { role: 'phone' }, secret)).status, 200);
  assert.equal((await call('/groups/00000000-0000-4000-8000-000000000001/leave', 'POST', {}, secret)).status, 200);
  assert.equal((await call('/edge/token', 'POST', { role: 'phone' }, secret)).status, 403);
});

test('a linked phone cannot ask to be a computer', async () => {
  const { call, join } = fixture();
  const { secret } = await join();
  const started = await call('/link/start', 'POST', {}, secret);
  const claim = await call('/link/claim', 'POST', { code: (started.body as { code: string }).code, device: 'Phone' });
  const phone = (claim.body as { secret: string }).secret;
  assert.equal((await call('/edge/token', 'POST', { role: 'computer' }, phone)).status, 403);
  assert.equal((await call('/edge/token', 'POST', { role: 'phone' }, phone)).status, 200);
  assert.equal((await call('/edge/token', 'POST', { role: 'computer' }, secret)).status, 200);
});

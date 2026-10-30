import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer, type IncomingMessage } from 'node:http';
import type { AddressInfo } from 'node:net';

test('Node HTTP adapter handles streaming and pre-parsed bodies through a complete local lifecycle', async t => {
  const invite = 'local-http-test-invite';
  process.env.SIPHON_FRIENDS_INVITE_CODE = invite;
  const { default: handler } = await import('../src/handler.ts');
  const server = createServer(async (req, res) => {
    if (req.headers['x-test-parsed'] === 'buffer') {
      const parts: Buffer[] = [];
      for await (const chunk of req) parts.push(Buffer.from(chunk));
      (req as IncomingMessage & { body?: unknown }).body = Buffer.concat(parts);
    }
    await handler(req, res);
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { server.closeAllConnections(); server.close(); delete process.env.SIPHON_FRIENDS_INVITE_CODE; });
  const base = `http://127.0.0.1:${(server.address() as AddressInfo).port}/api`;
  const call = async (path: string, method = 'GET', data?: unknown, secret?: string, parsed = false) => {
    const response = await fetch(`${base}${path}`, {
      method, headers: { 'content-type': 'application/json', ...(secret ? { authorization: `Bearer ${secret}` } : {}), ...(parsed ? { 'x-test-parsed': 'buffer' } : {}) },
      body: data === undefined ? undefined : JSON.stringify(data),
    });
    assert.equal(response.headers.get('cache-control'), 'no-store');
    return { status: response.status, body: await response.json() as any };
  };
  assert.equal((await call('/posts')).status, 401);
  const joined = await call('/join', 'POST', { invite_code: invite, display_name: 'HTTP listener' }, undefined, true);
  assert.equal(joined.status, 201);
  const { secret, user_id } = joined.body;
  assert.equal((await call('/me', 'GET', undefined, secret)).body.user_id, user_id);
  const privateUrl = 'https://soundcloud.com/friend/beat/s-private?secret_token=s-private';
  const created = await call('/posts', 'POST', { url: privateUrl, source: 'soundcloud', title: 'Private beat', artists: ['Friend'], artwork_url: null, duration_ms: 0 }, secret);
  assert.equal(created.status, 201);
  const id = created.body.id;
  assert.equal((await call(`/posts/${id}/like`, 'PUT', undefined, secret)).status, 200);
  let feed = await call('/posts', 'GET', undefined, secret);
  assert.equal(feed.body.posts[0].liked, true);
  assert.equal(feed.body.posts[0].url, privateUrl);
  assert.equal((await call('/me', 'PATCH', { display_name: 'Renamed' }, secret)).status, 200);
  feed = await call('/posts', 'GET', undefined, secret);
  assert.equal(feed.body.posts[0].author_name, 'Renamed');
  assert.equal((await call(`/posts/${id}/like`, 'DELETE', undefined, secret)).status, 200);
  assert.equal((await call(`/posts/${id}`, 'DELETE', undefined, secret)).status, 200);
  assert.deepEqual((await call('/posts', 'GET', undefined, secret)).body.posts, []);
  assert.equal((await call('/me', 'DELETE', undefined, secret)).status, 200);
  assert.equal((await call('/posts', 'GET', undefined, secret)).status, 401);
});

import assert from 'node:assert/strict';
import test from 'node:test';
import {
  createApi, Offline, RateLimited, Refused, SignedOut, TimedOut,
} from '../../assets/web/js/api.js';

const json = (value, status = 200, headers = {}) => new Response(JSON.stringify(value), {
  status,
  headers: { 'content-type': 'application/json', ...headers },
});

const apiWith = (fetch, extra = {}) => createApi({
  fetch,
  credentials: () => ({ secret: 'device-secret', group: 'group-1' }),
  sleep: async () => {},
  random: () => 0.5,
  ...extra,
});

test('a call carries the device secret, group and JSON body', async () => {
  const calls = [];
  const api = apiWith(async (url, init) => {
    calls.push({ url, init });
    return json({ ok: true });
  });

  const value = await api.post('/live/remote/send', { id: 'x', kind: 'next' });

  assert.deepEqual(value, { ok: true });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, '/api/live/remote/send');
  assert.equal(calls[0].init.method, 'POST');
  assert.equal(calls[0].init.headers.authorization, 'Bearer device-secret');
  assert.equal(calls[0].init.headers['x-siphon-group'], 'group-1');
  assert.equal(calls[0].init.headers['content-type'], 'application/json');
  assert.equal(calls[0].init.body, JSON.stringify({ id: 'x', kind: 'next' }));
});

test('no secret and no group means no auth headers', async () => {
  let seen;
  const api = apiWith(async (url, init) => {
    seen = init.headers;
    return json(null);
  }, { credentials: () => ({}) });

  assert.equal(await api.get('/me'), null);
  assert.equal(seen.authorization, undefined);
  assert.equal(seen['x-siphon-group'], undefined);
});

test('a GET retries a network failure and then answers', async () => {
  let calls = 0;
  const api = apiWith(async () => {
    calls += 1;
    if (calls < 3) throw new TypeError('network down');
    return json({ recovered: true });
  });

  assert.deepEqual(await api.get('/live/remote/state'), { recovered: true });
  assert.equal(calls, 3);
});

test('a POST is never retried', async () => {
  let calls = 0;
  const api = apiWith(async () => {
    calls += 1;
    throw new TypeError('network down');
  });

  await assert.rejects(api.post('/live/remote/send', { kind: 'next' }), error => {
    assert.ok(error instanceof Offline);
    assert.equal(error.name, 'Offline');
    return true;
  });
  assert.equal(calls, 1);
});

test('GET retries stop at their bound and report the last failure', async () => {
  let calls = 0;
  const api = apiWith(async () => {
    calls += 1;
    return json({ error: 'Still waking up' }, 503);
  }, { retries: 2 });

  await assert.rejects(api.get('/live/remote/state'), error => {
    assert.ok(error instanceof Refused);
    assert.equal(error.status, 503);
    assert.equal(error.message, 'Still waking up');
    return true;
  });
  assert.equal(calls, 3);
});

test('a slow service times out, and the timeout is its own error', async () => {
  let calls = 0;
  const api = apiWith((url, init) => {
    calls += 1;
    return new Promise((resolve, reject) => {
      init.signal.addEventListener('abort', () => reject(init.signal.reason), { once: true });
    });
  });

  await assert.rejects(api.get('/live/remote/state', { timeout: 20, retry: 0 }), error => {
    assert.ok(error instanceof TimedOut);
    assert.equal(error.timeout, 20);
    return true;
  });
  assert.equal(calls, 1);
});

test('a caller abort cancels the call and is not retried', async () => {
  let calls = 0;
  const api = apiWith((url, init) => {
    calls += 1;
    return new Promise((resolve, reject) => {
      init.signal.addEventListener('abort', () => reject(init.signal.reason), { once: true });
    });
  });
  const stop = new AbortController();

  const answer = api.get('/live/remote/state', { signal: stop.signal, retry: 2 });
  stop.abort();

  await assert.rejects(answer, error => {
    assert.equal(error.name, 'AbortError');
    return true;
  });
  assert.equal(calls, 1);
});

test('only the service saying "Sign in to the group" signs a device out', async () => {
  const signIn = apiWith(async () => json({ error: 'Sign in to the group' }, 401));
  await assert.rejects(signIn.get('/me'), error => error instanceof SignedOut);

  const other = apiWith(async () => json({ error: 'Not this door' }, 401));
  await assert.rejects(other.get('/me'), error => {
    assert.ok(error instanceof Refused);
    assert.equal(error.status, 401);
    assert.equal(error.message, 'Not this door');
    return true;
  });

  const proxy = apiWith(async () => new Response('<h1>Login</h1>', { status: 401 }));
  await assert.rejects(proxy.get('/me'), error => error instanceof Refused && !(error instanceof SignedOut));
});

test('a confirmation can keep a sign-in alive, or confirm its removal', async () => {
  const keep = apiWith(async () => json({ error: 'Sign in to the group' }, 401), {
    confirmSignedOut: async () => false,
  });
  await assert.rejects(keep.get('/me'), error => error instanceof Refused && !(error instanceof SignedOut));

  const reduce = apiWith(async () => json({ error: 'Sign in to the group' }, 401), {
    confirmSignedOut: async () => true,
  });
  await assert.rejects(reduce.get('/me'), error => error instanceof SignedOut);
});

test('a 401 does not hide a rate limit on another status', async () => {
  const api = apiWith(async () => json({ error: 'Slow down' }, 429, { 'retry-after': '1.5' }));
  await assert.rejects(api.get('/me', { retry: 0 }), error => {
    assert.ok(error instanceof RateLimited);
    assert.equal(error.status, 429);
    assert.equal(error.retryAfter, 1.5);
    return true;
  });
});

test('a rate limit longer than the wait ceiling is refused now', async () => {
  let calls = 0;
  let slept = 0;
  const api = apiWith(async () => {
    calls += 1;
    return json({ error: 'Later' }, 429, { 'retry-after': '60' });
  }, { sleep: async ms => { slept += ms; } });

  await assert.rejects(api.get('/me', { retry: 2, maxWait: 10000 }), error => error instanceof RateLimited);
  assert.equal(calls, 1);
  assert.equal(slept, 0);
});

test('a 500 is retried for a GET but not for a POST', async () => {
  let gets = 0;
  const getApi = apiWith(async () => {
    gets += 1;
    return gets < 3 ? json({ error: 'Down' }, 500) : json({ up: true });
  });
  assert.deepEqual(await getApi.get('/state'), { up: true });
  assert.equal(gets, 3);

  let posts = 0;
  const postApi = apiWith(async () => {
    posts += 1;
    return json({ error: 'Down' }, 500);
  });
  await assert.rejects(postApi.post('/send', {}), error => error instanceof Refused);
  assert.equal(posts, 1);
});

test('an unreadable successful answer is a refusal, not a crash', async () => {
  const api = apiWith(async () => new Response('not json', { status: 200 }));
  await assert.rejects(api.get('/me'), error => {
    assert.ok(error instanceof Refused);
    assert.equal(error.status, 200);
    return true;
  });
});

test('an empty answer is nothing', async () => {
  const api = apiWith(async () => new Response(null, { status: 204 }));
  assert.equal(await api.del('/live/remote/phone'), null);
});

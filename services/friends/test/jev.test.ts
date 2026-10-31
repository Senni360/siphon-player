import { test } from 'node:test';
import assert from 'node:assert/strict';
import { GATEWAY_ENDPOINT, TYPESAFE_ENDPOINT, createJev, titleReadings, type Ask, type Question } from '../src/jev.ts';
import { createService } from '../src/service.ts';
import { MemoryStorage } from '../src/storage.ts';

const never = new AbortController().signal;
const questions: Record<string, Question> = {
  enjoy: { type: 'noul', instructions: 'Would they enjoy it?' },
  pick: { type: 'choice', instructions: 'Which next?', criteria: { a: 'A', b: 'B' } },
};
function recorder(reply: () => Response) {
  const calls: { url: string; init: RequestInit }[] = [];
  const fetcher = (async (url: string, init: RequestInit) => { calls.push({ url, init }); return reply(); }) as unknown as typeof fetch;
  return { calls, fetcher };
}
const ok = (answers: unknown) => () => new Response(JSON.stringify({ model: 'jev-1.14.0', answers }), { status: 200 });

test('Jev stays silent without a credential and never calls out', async () => {
  const { calls, fetcher } = recorder(ok({}));
  assert.equal(await createJev({}, fetcher)('state', questions, new Headers(), never), null);
  assert.equal(calls.length, 0);
});

test('a TypeSafe key calls TypeSafe directly with the pinned model', async () => {
  const { calls, fetcher } = recorder(ok({
    enjoy: { type: 'noul', noul: 0.7 },
    pick: { type: 'choice', choice: 'a', confidence: 0.98, probabilities: { a: 0.99, b: 0.01, z: 0.5 } },
  }));
  const answers = await createJev({ TYPESAFE_API_KEY: 'ts-key', AI_GATEWAY_API_KEY: 'gw-key' }, fetcher)('state', questions, new Headers(), never);
  assert.deepEqual(answers, {
    enjoy: { type: 'noul', probability: 0.7 },
    pick: { type: 'choice', choice: 'a', probabilities: { a: 0.99, b: 0.01 } },
  });
  assert.equal(calls[0].url, TYPESAFE_ENDPOINT);
  assert.equal((calls[0].init.headers as Record<string, string>).Authorization, 'Bearer ts-key');
  assert.equal(JSON.parse(calls[0].init.body as string).model, 'jev-1.14.0');
});

test('without a TypeSafe key the gateway is used with the deployment OIDC token', async () => {
  const { calls, fetcher } = recorder(ok({ enjoy: { type: 'noul', noul: 0.2 } }));
  await createJev({}, fetcher)('state', questions, new Headers({ 'x-vercel-oidc-token': 'oidc' }), never);
  assert.equal(calls[0].url, GATEWAY_ENDPOINT);
  assert.equal((calls[0].init.headers as Record<string, string>).Authorization, 'Bearer oidc');
  assert.equal(JSON.parse(calls[0].init.body as string).model, 'typesafe-ai/jev');
});

test('failures and malformed answers read as no opinion', async () => {
  const env = { TYPESAFE_API_KEY: 'k' };
  assert.equal(await createJev(env, recorder(() => new Response('{"detail":"no"}', { status: 422 })).fetcher)('s', questions, new Headers(), never), null);
  assert.equal(await createJev(env, recorder(() => new Response('not json')).fetcher)('s', questions, new Headers(), never), null);
  assert.equal(await createJev(env, (async () => { throw new Error('offline'); }) as typeof fetch)('s', questions, new Headers(), never), null);
  const odd = await createJev(env, recorder(ok({ enjoy: { noul: 1.5 }, pick: { choice: 'zzz' } })).fetcher)('s', questions, new Headers(), never);
  assert.deepEqual(odd, {});
});

test('a slow Jev is abandoned rather than holding the request', async () => {
  const slow = ((_url: string, init: RequestInit) => new Promise((_, reject) => {
    init.signal!.addEventListener('abort', () => reject(init.signal!.reason));
  })) as unknown as typeof fetch;
  const caller = new AbortController();
  const pending = createJev({ TYPESAFE_API_KEY: 'k' }, slow)('s', questions, new Headers(), caller.signal);
  caller.abort();
  assert.equal(await pending, null);
});

test('upload titles become readings without noise, keeping a remix', () => {
  const readings = titleReadings('DJ Shadow - Midnight In A Perfect World (Prod. DJ Shadow) [FREE DL]', 'shadowfan99');
  assert.deepEqual(readings[0], { artist: 'DJ Shadow', title: 'Midnight In A Perfect World' });
  assert(readings.some(r => r.artist === 'shadowfan99'));
  assert.deepEqual(titleReadings('Song Name (Bicep Remix) | Artist', '')[1], { artist: 'Artist', title: 'Song Name (Bicep Remix)' });
  assert.deepEqual(titleReadings('Just A Song [Official Video]', 'Band - Topic'), [{ artist: 'Band', title: 'Just A Song' }]);
});

const invite = 'a-long-test-invite';
const share = (title: string, artist: string, extra = {}) => ({ url: `https://soundcloud.com/x/${title.toLowerCase().replace(/\W/g, '')}`, source: 'soundcloud', title, artists: [artist], artwork_url: null, duration_ms: 1000, ...extra });
function fixture(ask: Ask) {
  const store = new MemoryStorage();
  const service = createService(store, invite, ask);
  const call = async (path: string, method = 'GET', input?: unknown, secret?: string) => {
    const response = await service(new Request(`https://test/api${path}`, {
      method, headers: { 'content-type': 'application/json', ...(secret ? { authorization: `Bearer ${secret}` } : {}) },
      body: input === undefined ? undefined : JSON.stringify(input),
    }));
    return { status: response.status, body: await response.json() as any };
  };
  const join = async (name: string) => (await call('/join', 'POST', { invite_code: invite, display_name: name })).body as { user_id: string; secret: string };
  return { store, call, join };
}

test('each reader gets a cached taste score for friends\' shares', async () => {
  const asked: { state: string; questions: Record<string, Question> }[] = [];
  const ask: Ask = async (state, questions) => {
    asked.push({ state, questions });
    return Object.fromEntries(Object.keys(questions).map(id => [id, { type: 'noul', probability: 0.834 }]));
  };
  const { store, call, join } = fixture(ask);
  const a = await join('Alice');
  const b = await join('Bob');
  const friend = (await call('/posts', 'POST', share('Teardrop', 'Massive Attack'), b.secret)).body;
  // Alice has no taste yet: nothing to compare against, so no question.
  assert.equal((await call('/posts', 'GET', undefined, a.secret)).body.posts[0].affinity, null);
  assert.equal(asked.length, 0);
  await call('/posts', 'POST', share('Midnight City', 'M83'), a.secret);
  await call('/posts', 'POST', share('Intro', 'The xx'), a.secret);
  const feed = (await call('/posts', 'GET', undefined, a.secret)).body.posts;
  assert.equal(feed.find((p: any) => p.id === friend.id).affinity, 0.83);
  assert.equal(feed.filter((p: any) => p.affinity !== null).length, 1, 'own shares are never scored');
  assert.equal(asked.length, 1);
  assert.match(asked[0].state, /"Intro" by The xx\n"Midnight City" by M83/);
  await call('/posts', 'GET', undefined, a.secret);
  assert.equal(asked.length, 1, 'a score is asked for once');
  // Bob never sees Alice's score, and leaving removes hers.
  assert.equal((await call('/posts', 'GET', undefined, b.secret)).body.posts.find((p: any) => p.id === friend.id).affinity, null);
  await call('/me', 'DELETE', undefined, a.secret);
  assert.deepEqual((await store.read()).posts.find(p => p.id === friend.id)!.affinity, {});
});

test('the feed still loads when Jev has no answer', async () => {
  const { call, join } = fixture(async () => null);
  const a = await join('Alice');
  const b = await join('Bob');
  await call('/posts', 'POST', share('One', 'X'), a.secret);
  await call('/posts', 'POST', share('Two', 'Y'), a.secret);
  await call('/posts', 'POST', share('Three', 'Z'), b.secret);
  const feed = await call('/posts', 'GET', undefined, a.secret);
  assert.equal(feed.status, 200);
  assert(feed.body.posts.every((p: any) => p.affinity === null));
});

test('next-song ranking validates its input and returns Jev\'s probabilities', async () => {
  let criteria: Record<string, string> = {};
  const { call, join } = fixture(async (_state, questions) => {
    criteria = (questions.next as Extract<Question, { type: 'choice' }>).criteria;
    return { next: { type: 'choice', choice: 'spotify:track:a', probabilities: { 'spotify:track:a': 0.9, 'post:b': 0.1 } } };
  });
  const a = await join('Alice');
  const recent = [{ title: 'Midnight City', artists: ['M83'] }];
  const candidates = [{ id: 'spotify:track:a', title: 'Teardrop', artists: ['Massive Attack'] }, { id: 'post:b', title: 'Thunderstruck', artists: ['AC/DC'] }];
  const ranked = await call('/jev/next', 'POST', { recent, candidates }, a.secret);
  assert.deepEqual(ranked.body.probabilities, { 'spotify:track:a': 0.9, 'post:b': 0.1 });
  assert.equal(criteria['post:b'], '"Thunderstruck" by AC/DC');
  assert.equal((await call('/jev/next', 'POST', { recent, candidates: [candidates[0]] }, a.secret)).status, 400);
  assert.equal((await call('/jev/next', 'POST', { recent, candidates: [candidates[0], candidates[0]] }, a.secret)).status, 400);
  assert.equal((await call('/jev/next', 'POST', { recent, candidates: [{ ...candidates[0], id: 'bad id' }, candidates[1]] }, a.secret)).status, 400);
  assert.equal((await call('/jev/next', 'POST', { recent, candidates })).status, 401);
});

test('title cleanup applies only a confident pick and spends no question on a single reading', async () => {
  let count = 0;
  const { call, join } = fixture(async (_state, questions) => {
    count++;
    return Object.fromEntries(Object.keys(questions).map((id, i) => [id, { type: 'choice', choice: 'r0', probabilities: { r0: i === 0 ? 0.9 : 0.4 } }]));
  });
  const a = await join('Alice');
  const result = await call('/jev/titles', 'POST', { uploads: [
    { title: 'DJ Shadow - Midnight In A Perfect World [FREE DL]', uploader: 'fan' },
    { title: 'Kind of Blue - Miles', uploader: '' },
    { title: 'Just A Song (Official Video)', uploader: 'Band - Topic' },
  ] }, a.secret);
  assert.deepEqual(result.body.readings, [
    { artist: 'DJ Shadow', title: 'Midnight In A Perfect World' },
    null,
    { artist: 'Band', title: 'Just A Song' },
  ]);
  assert.equal(count, 1, 'every upload goes in one call');
});

test('a member cannot spend the key without bound', async () => {
  let count = 0;
  const { call, join } = fixture(async () => { count++; return null; });
  const a = await join('Alice');
  const body = { recent: [{ title: 'A', artists: [] }], candidates: [{ id: 'a', title: 'A', artists: [] }, { id: 'b', title: 'B', artists: [] }] };
  for (let i = 0; i < 31; i++) await call('/jev/next', 'POST', body, a.secret);
  assert.equal(count, 30);
});

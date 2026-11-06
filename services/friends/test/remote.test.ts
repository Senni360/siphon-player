import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createService } from '../src/service.js';
import { MemoryStorage } from '../src/storage.js';
import { codeMatches, gateCookie, normalized, passedGate, webAnswer } from '../src/web.js';

const invite = 'a-long-test-invite';
const ID = '11111111-1111-4111-8111-111111111111';

function fixture() {
  let time = 1_000_000;
  const store = new MemoryStorage(() => time);
  const service = createService(store, invite, undefined, () => time);
  const call = async (path: string, method = 'GET', input?: unknown, secret?: string, ip = 'a') => {
    const response = await service(new Request(`https://test/api${path}`, {
      method,
      headers: { 'content-type': 'application/json', ...(secret ? { authorization: `Bearer ${secret}` } : {}) },
      body: input === undefined ? undefined : JSON.stringify(input),
    }), ip);
    return { status: response.status, body: await response.json() as any };
  };
  const join = async (name: string) => (await call('/join', 'POST', { invite_code: invite, display_name: name })).body as { user_id: string; secret: string };
  /** An event stream, read one event at a time, until `close`. */
  const listen = async (side: 'computer' | 'phone', secret: string) => {
    const stop = new AbortController();
    const response = await service(new Request(`https://test/api/live/remote/events?for=${side}`, {
      headers: { authorization: `Bearer ${secret}` }, signal: stop.signal,
    }), 'a');
    assert.equal(response.status, 200);
    assert.match(response.headers.get('content-type') ?? '', /^text\/event-stream/);
    const reader = response.body!.pipeThrough(new TextDecoderStream()).getReader();
    let pending = '';
    const next = async (): Promise<any> => {
      while (true) {
        const end = pending.indexOf('\n\n');
        if (end >= 0) {
          const event = pending.slice(0, end);
          pending = pending.slice(end + 2);
          const data = event.split('\n').find(line => line.startsWith('data: '));
          if (data) return JSON.parse(data.slice(6));
          continue;
        }
        const part = await reader.read();
        if (part.done) return null;
        pending += part.value;
      }
    };
    return { next, close: () => { stop.abort(); void reader.cancel().catch(() => {}); } };
  };
  return { call, join, listen, advance: (ms: number) => { time += ms; } };
}

test('a phone joins the account of the computer that showed the code', async () => {
  const { call, join } = fixture();
  const computer = await join('Senni');
  const started = await call('/link/start', 'POST', { client_id: 'abc123' }, computer.secret);
  assert.equal(started.status, 201);
  assert.match(started.body.code, /^[2-9A-HJKMNP-Z]{8}$/);

  const claimed = await call('/link/claim', 'POST', { code: started.body.code.toLowerCase(), device: 'Phone' });
  assert.equal(claimed.status, 201);
  assert.equal(claimed.body.user_id, computer.user_id);
  assert.equal(claimed.body.client_id, 'abc123');
  assert.notEqual(claimed.body.secret, computer.secret);

  // The same account, on a secret of its own.
  assert.equal((await call('/me', 'GET', undefined, claimed.body.secret)).body.user_id, computer.user_id);
  const devices = (await call('/account', 'GET', undefined, computer.secret)).body.devices as { id: string; name: string }[];
  const phone = devices.find(device => device.name === 'Phone')!;
  assert.ok(phone);

  // Removing the phone on the computer leaves the computer signed in.
  assert.equal((await call(`/account/devices/${phone.id}`, 'DELETE', undefined, computer.secret)).status, 200);
  assert.equal((await call('/me', 'GET', undefined, claimed.body.secret)).status, 401);
  assert.equal((await call('/me', 'GET', undefined, computer.secret)).status, 200);
});

test('a link code works once, for a while, and a wrong one gets nothing', async () => {
  const { call, join, advance } = fixture();
  const computer = await join('Senni');
  assert.equal((await call('/link/start', 'POST', {})).status, 401);
  assert.equal((await call('/link/claim', 'POST', { code: 'WRONGONE' })).status, 404);
  const first = (await call('/link/start', 'POST', {}, computer.secret)).body.code;
  assert.equal((await call('/link/claim', 'POST', { code: first })).status, 201);
  assert.equal((await call('/link/claim', 'POST', { code: first })).status, 404, 'used once');
  const second = (await call('/link/start', 'POST', {}, computer.secret)).body.code;
  advance(301_000);
  assert.equal((await call('/link/claim', 'POST', { code: second })).status, 404, 'ran out');
});

test('commands reach the computer once and its answers reach the phone', async () => {
  const { call, join } = fixture();
  const computer = await join('Senni');
  const code = (await call('/link/start', 'POST', {}, computer.secret)).body.code;
  const phone = (await call('/link/claim', 'POST', { code })).body.secret;

  const idle = await call('/live/remote/poll', 'GET', undefined, computer.secret);
  assert.equal(idle.body.wanted, false);
  assert.deepEqual(idle.body.commands, []);
  assert.equal(idle.body.phone, null);

  assert.equal((await call('/live/remote/state', 'GET', undefined, phone)).body.state, null);
  const watched = await call('/live/remote/poll', 'GET', undefined, computer.secret);
  assert.equal(watched.body.wanted, true, 'a phone looking has the computer hurry');

  assert.equal((await call('/live/remote/send', 'POST', { id: ID, kind: 'next' }, phone)).status, 202);
  const polled = await call('/live/remote/poll', 'GET', undefined, computer.secret);
  assert.deepEqual(polled.body.commands, [{ id: ID, kind: 'next' }]);
  assert.deepEqual((await call('/live/remote/poll', 'GET', undefined, computer.secret)).body.commands, [], 'once');

  await call('/live/remote/state', 'PUT', { title: 'Song', playing: true }, computer.secret);
  const shown = await call('/live/remote/state', 'GET', undefined, phone);
  assert.equal(shown.body.state.title, 'Song');
  assert.ok(shown.body.state.at);

  assert.equal((await call(`/live/remote/reply/${ID}`, 'GET', undefined, phone)).body.ready, false);
  await call(`/live/remote/reply/${ID}`, 'PUT', { songs: [1] }, computer.secret);
  assert.deepEqual((await call(`/live/remote/reply/${ID}`, 'GET', undefined, phone)).body, { ready: true, reply: { songs: [1] } });
});

test('while both listen, a command, a new state and a ready answer arrive at once', async () => {
  const { call, join, listen } = fixture();
  const computer = await join('Senni');
  const code = (await call('/link/start', 'POST', {}, computer.secret)).body.code;
  const phone = (await call('/link/claim', 'POST', { code })).body.secret;
  await call('/live/remote/state', 'PUT', { title: 'Before', playing: true }, computer.secret);

  const toComputer = await listen('computer', computer.secret);
  const toPhone = await listen('phone', phone);
  try {
    assert.deepEqual((await toComputer.next()).phone, null, 'it opens with what the phone plays');
    const opening = await toPhone.next();
    assert.equal(opening.t, 'state');
    assert.equal(opening.state.title, 'Before', 'it opens with where things stand');
    assert.equal((await call('/live/remote/poll', 'GET', undefined, computer.secret)).body.wanted, true, 'a phone listening is watching');

    await call('/live/remote/send', 'POST', { id: ID, kind: 'toggle' }, phone);
    assert.deepEqual(await toComputer.next(), { t: 'command' });
    // The word only wakes it: the command itself waits in the inbox, as before.
    assert.deepEqual((await call('/live/remote/poll', 'GET', undefined, computer.secret)).body.commands, [{ id: ID, kind: 'toggle' }]);

    await call('/live/remote/state', 'PUT', { title: 'After', playing: false }, computer.secret);
    const changed = await toPhone.next();
    assert.equal(changed.t, 'state');
    assert.equal(changed.state.playing, false);

    await call(`/live/remote/reply/${ID}`, 'PUT', { songs: [] }, computer.secret);
    assert.deepEqual(await toPhone.next(), { t: 'reply', id: ID });
  } finally {
    toComputer.close();
    toPhone.close();
  }
});

test('the computer sees what the phone plays itself, and can work it', async () => {
  const { call, join, listen, advance } = fixture();
  const computer = await join('Senni');
  const code = (await call('/link/start', 'POST', {}, computer.secret)).body.code;
  const phone = (await call('/link/claim', 'POST', { code })).body.secret;
  const toComputer = await listen('computer', computer.secret);
  const toPhone = await listen('phone', phone);
  try {
    await toComputer.next();
    await toPhone.next();
    await call('/live/remote/phone', 'PUT', { song: { name: 'On the phone' }, playing: true, position_ms: 1000 }, phone);
    const heard = await toComputer.next();
    assert.equal(heard.t, 'phone');
    assert.equal(heard.phone.song.name, 'On the phone');
    assert.equal((await call('/live/remote/poll', 'GET', undefined, computer.secret)).body.phone.playing, true, 'a look finds it too');

    assert.equal((await call('/live/remote/control', 'POST', { kind: 'pause' }, computer.secret)).status, 202);
    assert.deepEqual(await toPhone.next(), { t: 'control', kind: 'pause' });
    assert.equal((await call('/live/remote/control', 'POST', { kind: 'seek', ms: 61000.4 }, computer.secret)).status, 202);
    assert.deepEqual(await toPhone.next(), { t: 'control', kind: 'seek', ms: 61000 });
    assert.equal((await call('/live/remote/control', 'POST', { kind: 'delete_everything' }, computer.secret)).status, 400);

    await call('/live/remote/phone', 'DELETE', undefined, phone);
    assert.deepEqual((await toComputer.next()).phone, null, 'stopping there says so');
    await call('/live/remote/phone', 'PUT', { playing: true }, phone);
    advance(121_000);
    assert.equal((await call('/live/remote/poll', 'GET', undefined, computer.secret)).body.phone, null, 'a phone gone quiet is forgotten');
  } finally {
    toComputer.close();
    toPhone.close();
  }
});

test('an event stream hears only its own account', async () => {
  const { call, join, listen } = fixture();
  const mine = await join('Senni');
  const theirs = await join('Friend');
  const mineHears = await listen('computer', mine.secret);
  try {
    await mineHears.next();
    await call('/live/remote/send', 'POST', { id: ID, kind: 'next' }, theirs.secret);
    await call('/live/remote/phone', 'PUT', { playing: true }, theirs.secret);
    await call('/live/remote/send', 'POST', { id: ID, kind: 'next' }, mine.secret);
    assert.deepEqual(await mineHears.next(), { t: 'command' }, 'the first word it hears is its own');
    assert.equal((await call('/live/remote/events?for=nobody', 'GET', undefined, mine.secret)).status, 400);
  } finally {
    mineHears.close();
  }
});

test('playing a long list fits in one command', async () => {
  const { call, join } = fixture();
  const { secret } = await join('Senni');
  const uris = Array.from({ length: 500 }, (_, index) => `soundcloud:track:${1000000000 + index}`);
  assert.equal((await call('/live/remote/send', 'POST', { id: ID, kind: 'play', uris, index: 0 }, secret)).status, 202);
  const polled = await call('/live/remote/poll', 'GET', undefined, secret);
  assert.equal(polled.body.commands[0].uris.length, 500);
});

test('one account can never reach another through the relay', async () => {
  const { call, join } = fixture();
  const mine = await join('Senni');
  const theirs = await join('Friend');
  assert.equal((await call('/live/remote/send', 'POST', { id: ID, kind: 'next' })).status, 401);
  await call('/live/remote/send', 'POST', { id: ID, kind: 'next' }, theirs.secret);
  assert.deepEqual((await call('/live/remote/poll', 'GET', undefined, mine.secret)).body.commands, []);
  await call('/live/remote/state', 'PUT', { title: 'Private' }, mine.secret);
  assert.equal((await call('/live/remote/state', 'GET', undefined, theirs.secret)).body.state, null);
  await call(`/live/remote/reply/${ID}`, 'PUT', { songs: [1] }, mine.secret);
  assert.equal((await call(`/live/remote/reply/${ID}`, 'GET', undefined, theirs.secret)).body.ready, false);
});

test('a command that is not a command is refused', async () => {
  const { call, join } = fixture();
  const { secret } = await join('Senni');
  assert.equal((await call('/live/remote/send', 'POST', { id: 'nope', kind: 'next' }, secret)).status, 400);
  assert.equal((await call('/live/remote/send', 'POST', { id: ID, kind: 'Bad Kind!' }, secret)).status, 400);
});

const code = 'test words';
const limits = (allowed = true) => ({ limit: async () => allowed });
const nothing = async () => ({});

test('the code forgives case and spaces, and nothing else', () => {
  assert.equal(normalized('  Test   WORDS '), 'testwords');
  assert.ok(codeMatches('TEST WORDS', code));
  assert.ok(codeMatches('testwords', code));
  assert.ok(!codeMatches('test word', code));
  assert.ok(!codeMatches('anything', ''), 'no code set lets nobody in');
});

test('the gate cookie is signed, expires and needs the current code', () => {
  const cookie = gateCookie(code, 1000).split(';')[0];
  assert.ok(passedGate(`other=1; ${cookie}`, code, 2000));
  assert.ok(!passedGate(cookie, 'another code', 2000), 'changing the code locks everyone out');
  assert.ok(!passedGate(cookie, code, 1000 + 366 * 86400 * 1000), 'expired');
  assert.ok(!passedGate(cookie.replace(/\.[a-f0-9]{4}/, '.0000'), code, 2000), 'forged');
  assert.ok(!passedGate(undefined, code, 2000));
  assert.ok(!passedGate('encore_gate=abc', code, 2000));
});

test('the web address shows only the gate until the code is in', async () => {
  const ask = (method: string, path: string, cookie?: string, input: () => Promise<unknown> = nothing, allowed = true, set = code) =>
    webAnswer(method, path, cookie, input, set, limits(allowed), 'ip', 1000);
  const gate = await ask('GET', '/');
  assert.match(String(gate!.body), /encore-gate/);
  assert.equal((await ask('GET', '/app.js'))!.status, 401, 'the app itself stays behind it');
  assert.equal(await ask('GET', '/api/me'), null, 'the API answers for itself');

  const wrong = await ask('POST', '/gate', undefined, async () => ({ code: 'wrong' }));
  assert.equal(wrong!.status, 403);
  assert.equal(wrong!.headers['Set-Cookie'], undefined);
  assert.equal((await ask('POST', '/gate', undefined, async () => ({ code: 'x' }), false))!.status, 429);
  assert.equal((await ask('POST', '/gate', undefined, async () => ({ code }), true, ''))!.status, 503, 'closed until set up');

  const right = await ask('POST', '/gate', undefined, async () => ({ code: 'TEST WORDS' }));
  assert.equal(right!.status, 200);
  const cookie = right!.headers['Set-Cookie'].split(';')[0];
  assert.match(right!.headers['Set-Cookie'], /HttpOnly; Secure; SameSite=Lax/);
  const app = await ask('GET', '/', cookie);
  assert.match(String(app!.body), /id="app"/);
  assert.equal((await ask('GET', '/app.js', cookie))!.status, 200);
  assert.match(String((await ask('GET', '/callback', cookie))!.body), /id="app"/, 'Spotify can return here');
});

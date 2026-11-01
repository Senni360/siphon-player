import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createService } from '../src/service.js';
import { MemoryStorage } from '../src/storage.js';

const INVITE = 'siphon-local-test-invite';
const song = (id: string, duration_ms = 180000) => ({ url: `https://open.spotify.com/track/${id}`, source: 'spotify', title: id, artists: ['Artist'], artwork_url: null, duration_ms });

function group() {
  let now = 1_000_000;
  const storage = new MemoryStorage(() => now);
  const service = createService(storage, INVITE, undefined, () => now);
  const call = async (secret: string | null, method: string, path: string, data?: unknown) => {
    const response = await service(new Request(`http://localhost/api${path}`, {
      method,
      headers: { ...(secret ? { Authorization: `Bearer ${secret}` } : {}), ...(data === undefined ? {} : { 'Content-Type': 'application/json' }) },
      ...(data === undefined ? {} : { body: JSON.stringify(data) }),
    }));
    return { status: response.status, body: await response.json() as any };
  };
  const join = async (display_name: string) => {
    const { body } = await call(null, 'POST', '/join', { invite_code: INVITE, display_name });
    return { id: body.user_id as string, secret: body.secret as string };
  };
  return { call, join, advance: (ms: number) => { now += ms; }, now: () => now };
}

test('live routes need a member', async () => {
  const { call } = group();
  assert.equal((await call(null, 'GET', '/live')).status, 401);
  assert.equal((await call('a'.repeat(64), 'GET', '/live')).status, 401);
});

test('presence shows what a member plays, carries the server time, and expires by itself', async () => {
  const { call, join, advance, now } = group();
  const a = await join('A'); const b = await join('B');
  const put = await call(a.secret, 'PUT', '/live/presence', { item: song('one'), position_ms: 5000, playing: true });
  assert.equal(put.status, 200);
  const seen = await call(b.secret, 'GET', '/live');
  assert.equal(seen.body.now, now());
  assert.equal(seen.body.presence.length, 1);
  assert.equal(seen.body.presence[0].user_id, a.id);
  assert.equal(seen.body.presence[0].item.title, 'one');
  assert.equal(seen.body.presence[0].at, now());
  advance(201000);
  assert.deepEqual((await call(b.secret, 'GET', '/live')).body.presence, []);
});

test('clearing presence and following are recorded for the member alone', async () => {
  const { call, join } = group();
  const a = await join('A'); const b = await join('B');
  await call(a.secret, 'PUT', '/live/presence', { item: song('one'), position_ms: 0, playing: true });
  await call(b.secret, 'PUT', '/live/presence', { item: song('one'), position_ms: 0, playing: true, following: a.id });
  const both = (await call(a.secret, 'GET', '/live')).body.presence;
  assert.equal(both.find((p: any) => p.user_id === b.id).following, a.id);
  assert.equal((await call(a.secret, 'PUT', '/live/presence', { item: null, following: a.id })).status, 400);
  await call(a.secret, 'DELETE', '/live/presence');
  assert.deepEqual((await call(b.secret, 'GET', '/live')).body.presence.map((p: any) => p.user_id), [b.id]);
});

test('presence refuses anything but one playable song', async () => {
  const { call, join } = group();
  const a = await join('A');
  for (const item of [
    { ...song('x'), url: 'https://open.spotify.com/album/x' },
    { ...song('x'), url: 'https://soundcloud.com/a/sets/b', source: 'soundcloud' },
    { ...song('x'), url: 'https://evil.example/track/x' },
    { ...song('x'), title: '' },
  ]) assert.equal((await call(a.secret, 'PUT', '/live/presence', { item })).status, 400);
  const sc = { ...song('x'), url: 'https://soundcloud.com/a/b/s-Secret1', source: 'soundcloud' };
  assert.equal((await call(a.secret, 'PUT', '/live/presence', { item: sc, position_ms: 0, playing: true })).status, 200);
  const yt = { ...song('x'), url: 'https://www.youtube.com/watch?v=dQw4w9WgXcQ', source: 'youtube' };
  assert.equal((await call(a.secret, 'PUT', '/live/presence', { item: yt, position_ms: 0, playing: true })).status, 200);
});

test('in a channel everyone changes the queue and what plays', async () => {
  const { call, join, advance, now } = group();
  const a = await join('A'); const b = await join('B');
  const made = await call(a.secret, 'POST', '/live/channels', { name: 'Friday', item: song('one'), position_ms: 1000, playing: true });
  assert.equal(made.status, 201);
  const id = made.body.channel.id;
  assert.equal(made.body.channel.current.item.title, 'one');
  // Not in the channel yet: looking is fine, changing is not.
  assert.equal((await call(b.secret, 'POST', `/live/channels/${id}/queue`, { item: song('two') })).status, 403);
  assert.equal((await call(b.secret, 'POST', `/live/channels/${id}/join`)).body.channel.members.length, 2);
  const two = (await call(b.secret, 'POST', `/live/channels/${id}/queue`, { item: song('two') })).body.channel.queue[0];
  const three = (await call(a.secret, 'POST', `/live/channels/${id}/queue`, { item: song('three') })).body.channel.queue[1];
  assert.equal(two.added_by, b.id);
  await call(a.secret, 'POST', `/live/channels/${id}/queue`, { item: song('front'), where: 'next' });
  let channel = (await call(b.secret, 'POST', `/live/channels/${id}/queue/${three.id}/move`, { index: 0 })).body.channel;
  assert.deepEqual(channel.queue.map((e: any) => e.item.title), ['three', 'front', 'two']);
  channel = (await call(b.secret, 'DELETE', `/live/channels/${id}/queue/${channel.queue[1].id}`)).body.channel;
  assert.deepEqual(channel.queue.map((e: any) => e.item.title), ['three', 'two']);
  advance(1000);
  channel = (await call(b.secret, 'POST', `/live/channels/${id}/playback`, { playing: false, position_ms: 42000 })).body.channel;
  assert.deepEqual([channel.playing, channel.position_ms, channel.at], [false, 42000, now()]);
  channel = (await call(a.secret, 'POST', `/live/channels/${id}/next`, {})).body.channel;
  assert.deepEqual([channel.current.item.title, channel.playing, channel.position_ms], ['three', true, 0]);
  channel = (await call(a.secret, 'POST', `/live/channels/${id}/queue`, { item: song('now'), where: 'now' })).body.channel;
  assert.equal(channel.current.item.title, 'now');
  assert.deepEqual(channel.queue.map((e: any) => e.item.title), ['two']);
});

test('every listener reporting a song\'s end moves on once', async () => {
  const { call, join } = group();
  const a = await join('A'); const b = await join('B');
  const made = (await call(a.secret, 'POST', '/live/channels', { name: 'C', item: song('one') })).body.channel;
  await call(b.secret, 'POST', `/live/channels/${made.id}/join`);
  await call(a.secret, 'POST', `/live/channels/${made.id}/queue`, { item: song('two') });
  await call(a.secret, 'POST', `/live/channels/${made.id}/queue`, { item: song('three') });
  const first = (await call(a.secret, 'POST', `/live/channels/${made.id}/next`, { from: made.current.id })).body.channel;
  const second = (await call(b.secret, 'POST', `/live/channels/${made.id}/next`, { from: made.current.id })).body.channel;
  assert.equal(first.current.item.title, 'two');
  assert.equal(second.current.item.title, 'two');
  // Choosing a row skips the rows above it, as the queue's own rules say.
  const skipped = (await call(a.secret, 'POST', `/live/channels/${made.id}/next`, { to: second.queue[0].id })).body.channel;
  assert.equal(skipped.current.item.title, 'three');
  assert.deepEqual(skipped.queue, []);
  const ended = (await call(a.secret, 'POST', `/live/channels/${made.id}/next`, {})).body.channel;
  assert.deepEqual([ended.current, ended.playing], [null, false]);
  // An empty channel plays what is added to it straight away.
  const again = (await call(a.secret, 'POST', `/live/channels/${made.id}/queue`, { item: song('four') })).body.channel;
  assert.deepEqual([again.current.item.title, again.playing, again.queue.length], ['four', true, 0]);
});

test('a member is in one channel, and a channel ends with its last member', async () => {
  const { call, join, advance } = group();
  const a = await join('A'); const b = await join('B');
  const one = (await call(a.secret, 'POST', '/live/channels', { name: 'One' })).body.channel;
  const two = (await call(b.secret, 'POST', '/live/channels', { name: 'Two' })).body.channel;
  await call(a.secret, 'POST', `/live/channels/${two.id}/join`);
  let channels = (await call(a.secret, 'GET', '/live')).body.channels;
  assert.deepEqual(channels.map((c: any) => [c.name, c.members.length]), [['Two', 2]]);
  assert.equal((await call(a.secret, 'POST', `/live/channels/${one.id}/join`)).status, 404);
  await call(b.secret, 'POST', `/live/channels/${two.id}/leave`);
  channels = (await call(a.secret, 'GET', '/live')).body.channels;
  assert.deepEqual(channels[0].members, [a.id]);
  // Gone without saying so: when the app closes, presence runs out.
  advance(201000);
  assert.deepEqual((await call(b.secret, 'GET', '/live')).body.channels, []);
});

test('leaving the group leaves nothing live behind', async () => {
  const { call, join } = group();
  const a = await join('A'); const b = await join('B');
  const made = (await call(a.secret, 'POST', '/live/channels', { name: 'C', item: song('one') })).body.channel;
  await call(b.secret, 'POST', `/live/channels/${made.id}/join`);
  await call(a.secret, 'PUT', '/live/presence', { item: song('one'), position_ms: 0, playing: true });
  assert.equal((await call(a.secret, 'DELETE', '/me')).status, 200);
  const live = (await call(b.secret, 'GET', '/live')).body;
  assert.deepEqual(live.channels[0].members, [b.id]);
  assert.ok(!live.presence.some((p: any) => p.user_id === a.id));
  assert.equal((await call(a.secret, 'GET', '/live')).status, 401);
});

test('a live read costs no read of the group document once known', async () => {
  let now = 1_000_000;
  class Counting extends MemoryStorage {
    reads = 0; limitCalls = 0;
    override async read(signal?: AbortSignal) { this.reads++; return super.read(signal); }
    override async limit(key: string, maximum: number, seconds: number, signal?: AbortSignal) { this.limitCalls++; return super.limit(key, maximum, seconds, signal); }
  }
  const storage = new Counting(() => now);
  const service = createService(storage, INVITE, undefined, () => now);
  const joined = await (await service(new Request('http://localhost/api/join', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ invite_code: INVITE, display_name: 'A' }) }))).json() as any;
  const read = () => service(new Request('http://localhost/api/live', { headers: { Authorization: `Bearer ${joined.secret}` } }));
  await read();
  const [reads, limits] = [storage.reads, storage.limitCalls];
  for (let i = 0; i < 5; i++) assert.equal((await read()).status, 200);
  assert.deepEqual([storage.reads, storage.limitCalls], [reads, limits]);
  now += 61000;
  await read();
  assert.equal(storage.reads, reads + 1);
});

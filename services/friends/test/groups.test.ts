import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createService, type VerifySpotify } from '../src/service.js';
import { MemoryStorage } from '../src/storage.js';
import { FIRST_GROUP } from '../src/groups.js';

const INVITE = 'siphon-local-test-invite';
const song = (id: string, duration_ms = 180000) => ({ url: `https://open.spotify.com/track/${id}`, source: 'spotify', title: id, artists: ['Artist'], artwork_url: null, duration_ms });

function world(ownerSpotify = '') {
  let now = 1_000_000;
  const seen: string[] = [];
  // A token here is just the Spotify id it proves. Anything starting with bad is refused.
  const verify: VerifySpotify = async token => {
    seen.push(token);
    if (token.startsWith('bad')) { const { fail } = await import('../src/http.js'); return fail(401, 'Spotify did not accept this sign-in'); }
    return { id: token, display_name: token.toUpperCase() };
  };
  const storage = new MemoryStorage(() => now);
  const service = createService(storage, INVITE, undefined, () => now, verify, ownerSpotify);
  const call = async (secret: string | null, method: string, path: string, data?: unknown, group?: string) => {
    const response = await service(new Request(`http://localhost/api${path}`, {
      method,
      headers: {
        ...(secret ? { Authorization: `Bearer ${secret}` } : {}),
        ...(data === undefined ? {} : { 'Content-Type': 'application/json' }),
        ...(group ? { 'X-Siphon-Group': group } : {}),
      },
      ...(data === undefined ? {} : { body: JSON.stringify(data) }),
    }));
    return { status: response.status, body: await response.json() as any };
  };
  const signIn = async (token: string, device = 'Desk', held: string | null = null) => {
    const { status, body } = await call(held, 'POST', '/auth/spotify', { token, device });
    assert.equal(status, 201, JSON.stringify(body));
    return { id: body.user_id as string, secret: body.secret as string, name: body.display_name as string };
  };
  const legacy = async (display_name: string) => {
    const { body } = await call(null, 'POST', '/join', { invite_code: INVITE, display_name });
    return { id: body.user_id as string, secret: body.secret as string };
  };
  return { call, signIn, legacy, seen, storage, advance: (ms: number) => { now += ms; } };
}

test('signing in with Spotify on a second computer is the same account in the same groups', async () => {
  const { call, signIn } = world();
  const desk = await signIn('senni', 'Desk');
  const { body: group } = await call(desk.secret, 'POST', '/groups', { name: 'Studio' });
  const laptop = await signIn('senni', 'Laptop');
  assert.equal(laptop.id, desk.id);
  assert.notEqual(laptop.secret, desk.secret);
  const { body: account } = await call(laptop.secret, 'GET', '/account');
  assert.deepEqual(account.groups.map((g: any) => g.id), [group.id]);
  assert.deepEqual(account.devices.map((d: any) => [d.name, d.current]), [['Desk', false], ['Laptop', true]]);
});

test('a token Spotify refuses signs nobody in, and is never kept', async () => {
  const { call, signIn, storage } = world();
  assert.equal((await call(null, 'POST', '/auth/spotify', { token: 'bad-token' })).status, 401);
  await signIn('senni');
  assert(!JSON.stringify(await storage.read()).includes('Bearer'));
  const state = await storage.read();
  assert.equal(state.users.length, 1);
  assert.equal(state.users[0].spotify_id, 'senni');
});

test('a removed device stops working at once', async () => {
  const { call, signIn } = world();
  const desk = await signIn('senni', 'Desk');
  const laptop = await signIn('senni', 'Laptop');
  const { body } = await call(desk.secret, 'GET', '/account');
  const other = body.devices.find((d: any) => !d.current);
  assert.equal((await call(desk.secret, 'DELETE', `/account/devices/${other.id}`)).status, 200);
  assert.equal((await call(laptop.secret, 'GET', '/account')).status, 401);
});

test('a device in use is never the one that goes when the account is full', async () => {
  const { call, signIn, advance } = world();
  // Twenty devices fit. Each is added an hour after the one before.
  const devices: { secret: string }[] = [];
  for (let i = 0; i < 21; i++) {
    advance(3600_000);
    devices.push(await signIn('senni', `Device${i}`));
  }
  // The twenty-first arrived: the earliest, never used, is the one that went.
  assert.equal((await call(devices[0].secret, 'GET', '/me')).status, 401);
  assert.equal((await call(devices[1].secret, 'GET', '/me')).status, 200);

  // A day and an hour later, the second device is used again, and then one
  // more arrives: the one that goes is the least recently seen, not the used one.
  advance(25 * 3600_000);
  assert.equal((await call(devices[1].secret, 'GET', '/me')).status, 200);
  advance(3600_000);
  devices.push(await signIn('senni', 'Device21'));
  assert.equal((await call(devices[1].secret, 'GET', '/me')).status, 200, 'the used one survives');
  assert.equal((await call(devices[2].secret, 'GET', '/me')).status, 401, 'the one seen longest ago goes');
  assert.equal((await call(devices[20].secret, 'GET', '/me')).status, 200, 'the recent arrivals survive');
  const { body: account } = await call(devices[20].secret, 'GET', '/account');
  assert.equal(account.devices.length, 20, 'the account is bounded');
});

test('a member from before accounts keeps everything when they sign in, and two identities fold into one', async () => {
  const { call, signIn, legacy } = world();
  const first = await legacy('Senni');
  const second = await legacy('Senni laptop');
  const { body: mine } = await call(first.secret, 'POST', '/posts', { url: 'https://open.spotify.com/track/abc', source: 'spotify', title: 'T', artists: [], artwork_url: null, duration_ms: 0 });
  const { body: theirs } = await call(second.secret, 'POST', '/posts', { url: 'https://open.spotify.com/track/def', source: 'spotify', title: 'U', artists: [], artwork_url: null, duration_ms: 0 });
  await call(second.secret, 'PUT', `/posts/${mine.id}/like`);

  const linked = await signIn('senni', 'Desk', first.secret);
  assert.equal(linked.id, first.id, 'their first sign-in keeps their identity');
  const folded = await signIn('senni', 'Laptop', second.secret);
  assert.equal(folded.id, first.id, 'the second identity folds into the account');

  const feed = (await call(folded.secret, 'GET', '/posts')).body.posts;
  assert.deepEqual(feed.map((p: any) => p.author_id).sort(), [first.id, first.id]);
  assert.equal(feed.find((p: any) => p.id === theirs.id).author_name, 'Senni');
  assert.equal(feed.find((p: any) => p.id === mine.id).like_count, 1);
  const members = (await call(folded.secret, 'GET', '/members')).body.members;
  assert.equal(members.length, 1);
  // The old client on that laptop, still holding the old secret, goes on working.
  assert.equal((await call(second.secret, 'GET', '/me')).body.user_id, first.id);
});

test('a client from before groups sees exactly the first group, and nothing of the others', async () => {
  const { call, signIn, legacy } = world();
  const old = await legacy('Old client');
  const creator = await signIn('senni');
  const { body: studio } = await call(creator.secret, 'POST', '/groups', { name: 'Studio' });
  const shared = await call(creator.secret, 'POST', '/posts', { url: 'https://open.spotify.com/track/abc', source: 'spotify', title: 'Hidden', artists: [], artwork_url: null, duration_ms: 0 }, studio.id);
  assert.equal(shared.status, 201);
  assert.deepEqual((await call(old.secret, 'GET', '/posts')).body.posts, []);
  assert.equal((await call(old.secret, 'GET', '/posts', undefined, studio.id)).status, 403);
  assert.equal((await call(old.secret, 'GET', '/live', undefined, studio.id)).status, 403);
  assert.equal((await call(old.secret, 'PUT', `/posts/${shared.body.id}/like`)).status, 404);
  // Someone with no seat in the first group is refused there, not given an empty one.
  assert.equal((await call(creator.secret, 'GET', '/posts')).status, 403);
});

test('the deployment names who owns the first group, and only that account takes it', async () => {
  const { call, signIn, legacy } = world('senni');
  const stranger = await legacy('Stranger');
  const me = await legacy('Senni');
  await signIn('stranger', 'Desk', stranger.secret);
  let group = (await call(stranger.secret, 'GET', '/account')).body.groups[0];
  assert.equal(group.owner_id, null);
  const signed = await signIn('senni', 'Desk', me.secret);
  group = (await call(signed.secret, 'GET', '/account')).body.groups[0];
  assert.equal(group.owner_id, me.id);
  assert.equal(group.role, 'owner');
});

test('the creator runs the group: new codes, removing, adding and rights', async () => {
  const { call, signIn } = world();
  const owner = await signIn('owner');
  const { body: created } = await call(owner.secret, 'POST', '/groups', { name: 'Studio' });
  assert.equal(created.owner_id, owner.id);
  assert.equal(created.role, 'owner');
  const firstCode = created.invite as string;
  assert(firstCode.length >= 12);

  const guest = await signIn('guest');
  assert.equal((await call(guest.secret, 'POST', '/groups/join', { code: 'nope' })).status, 403);
  const joined = await call(guest.secret, 'POST', '/groups/join', { code: firstCode });
  assert.equal(joined.body.role, 'member');
  assert.equal(joined.body.invite, null, 'a member does not see the code');

  // Nothing here belongs to a plain member.
  assert.equal((await call(guest.secret, 'POST', `/groups/${created.id}/invite`)).status, 403);
  assert.equal((await call(guest.secret, 'DELETE', `/groups/${created.id}/members/${owner.id}`)).status, 403);
  assert.equal((await call(guest.secret, 'PATCH', `/groups/${created.id}`, { name: 'Mine' })).status, 403);
  assert.equal((await call(guest.secret, 'DELETE', `/groups/${created.id}`)).status, 403);

  // A right the owner hands out is enforced by the service, not just hidden.
  const song2 = { url: 'https://open.spotify.com/track/abc', source: 'spotify', title: 'T', artists: [], artwork_url: null, duration_ms: 0 };
  assert.equal((await call(guest.secret, 'POST', '/posts', song2, created.id)).status, 201);
  await call(owner.secret, 'PATCH', `/groups/${created.id}/members/${guest.id}`, { rights: { share: false } });
  assert.equal((await call(guest.secret, 'POST', '/posts', song2, created.id)).status, 403);
  await call(owner.secret, 'PATCH', `/groups/${created.id}/members/${guest.id}`, { rights: { share: true, invite: true } });
  const rotated = await call(guest.secret, 'POST', `/groups/${created.id}/invite`);
  assert.equal(rotated.status, 200);
  assert.notEqual(rotated.body.invite, firstCode);

  // A new code ends the old one, and a removal outlives any code.
  const late = await signIn('late');
  assert.equal((await call(late.secret, 'POST', '/groups/join', { code: firstCode })).status, 403);
  assert.equal((await call(owner.secret, 'DELETE', `/groups/${created.id}/members/${guest.id}`)).status, 200);
  assert.equal((await call(guest.secret, 'GET', '/posts', undefined, created.id)).status, 403);
  assert.equal((await call(guest.secret, 'POST', '/groups/join', { code: rotated.body.invite })).status, 403);
  assert.equal((await call(owner.secret, 'POST', `/groups/${created.id}/members`, { spotify_id: 'guest' })).status, 201);
  assert.equal((await call(guest.secret, 'GET', '/posts', undefined, created.id)).status, 200);
});

test('an admin added ahead of time is in the group the moment they sign in', async () => {
  const { call, signIn } = world();
  const owner = await signIn('owner');
  const { body: group } = await call(owner.secret, 'POST', '/groups', { name: 'Studio' });
  const added = await call(owner.secret, 'POST', `/groups/${group.id}/members`, { spotify_id: 'https://open.spotify.com/user/newcomer?si=abc' });
  assert.deepEqual(added.body.pending, ['newcomer']);
  const newcomer = await signIn('newcomer');
  const { body: account } = await call(newcomer.secret, 'GET', '/account');
  assert.deepEqual(account.groups.map((g: any) => g.id), [group.id]);
  assert.deepEqual(account.groups[0].pending, []);
});

test('an admin may not touch the owner or another admin, and only the owner makes admins', async () => {
  const { call, signIn } = world();
  const owner = await signIn('owner');
  const { body: group } = await call(owner.secret, 'POST', '/groups', { name: 'Studio' });
  const one = await signIn('one'); const two = await signIn('two');
  await call(one.secret, 'POST', '/groups/join', { code: group.invite });
  await call(two.secret, 'POST', '/groups/join', { code: group.invite });
  await call(owner.secret, 'PATCH', `/groups/${group.id}/members/${one.id}`, { role: 'admin' });
  await call(owner.secret, 'PATCH', `/groups/${group.id}/members/${two.id}`, { role: 'admin' });
  assert.equal((await call(one.secret, 'DELETE', `/groups/${group.id}/members/${owner.id}`)).status, 403);
  assert.equal((await call(one.secret, 'DELETE', `/groups/${group.id}/members/${two.id}`)).status, 403);
  assert.equal((await call(one.secret, 'PATCH', `/groups/${group.id}/members/${two.id}`, { rights: { share: false } })).status, 403);
  await call(owner.secret, 'PATCH', `/groups/${group.id}/members/${two.id}`, { role: 'member' });
  assert.equal((await call(one.secret, 'PATCH', `/groups/${group.id}/members/${two.id}`, { role: 'admin' })).status, 403);
  assert.equal((await call(one.secret, 'DELETE', `/groups/${group.id}/members/${two.id}`)).status, 200);
});

test('when the owner leaves, the group falls to its admin; the last to leave deletes it', async () => {
  const { call, signIn } = world();
  const owner = await signIn('owner');
  const { body: group } = await call(owner.secret, 'POST', '/groups', { name: 'Studio' });
  const heir = await signIn('heir');
  await call(heir.secret, 'POST', '/groups/join', { code: group.invite });
  await call(owner.secret, 'POST', `/groups/${group.id}/leave`);
  const { body: account } = await call(heir.secret, 'GET', '/account');
  assert.equal(account.groups[0].role, 'owner');
  await call(heir.secret, 'POST', `/groups/${group.id}/leave`);
  assert.deepEqual((await call(heir.secret, 'GET', '/account')).body.groups, []);
});

test('the first group cannot be deleted, so old clients always have somewhere to be', async () => {
  const { call, signIn, legacy } = world('senni');
  const me = await legacy('Senni');
  const signed = await signIn('senni', 'Desk', me.secret);
  assert.equal((await call(signed.secret, 'DELETE', `/groups/${FIRST_GROUP}`)).status, 409);
});

test('SoundCloud links to the account as a label and comes off again', async () => {
  const { call, signIn } = world();
  const me = await signIn('senni');
  await call(me.secret, 'PUT', '/account/soundcloud', { id: '42', username: 'senni-beats' });
  assert.deepEqual((await call(me.secret, 'GET', '/account')).body.soundcloud, { id: '42', username: 'senni-beats' });
  await call(me.secret, 'DELETE', '/account/soundcloud');
  assert.equal((await call(me.secret, 'GET', '/account')).body.soundcloud, null);
});

test('a shared playlist reaches the group, opens with its songs, and goes when deleted', async () => {
  const { call, signIn } = world();
  const owner = await signIn('owner');
  const { body: group } = await call(owner.secret, 'POST', '/groups', { name: 'Studio' });
  const friend = await signIn('friend'); const outsider = await signIn('outsider');
  await call(friend.secret, 'POST', '/groups/join', { code: group.invite });
  const posted = await call(owner.secret, 'POST', '/playlists', { name: 'Late', items: [song('one'), song('two')] }, group.id);
  assert.equal(posted.status, 201);
  assert.equal(posted.body.count, 2);
  const list = await call(friend.secret, 'GET', '/playlists', undefined, group.id);
  assert.deepEqual(list.body.playlists.map((p: any) => [p.name, p.by_name, p.count]), [['Late', 'OWNER', 2]]);
  const opened = await call(friend.secret, 'GET', `/playlists/${posted.body.id}`, undefined, group.id);
  assert.deepEqual(opened.body.items.map((s: any) => s.title), ['one', 'two']);
  assert.equal((await call(outsider.secret, 'GET', `/playlists/${posted.body.id}`, undefined, group.id)).status, 403);
  assert.equal((await call(friend.secret, 'DELETE', `/playlists/${posted.body.id}`, undefined, group.id)).status, 403);
  assert.equal((await call(owner.secret, 'DELETE', `/playlists/${posted.body.id}`, undefined, group.id)).status, 200);
  assert.deepEqual((await call(friend.secret, 'GET', '/playlists', undefined, group.id)).body.playlists, []);
});

test('a playlist sent to one friend is seen only by them', async () => {
  const { call, signIn } = world();
  const owner = await signIn('owner');
  const { body: group } = await call(owner.secret, 'POST', '/groups', { name: 'Studio' });
  const one = await signIn('one'); const two = await signIn('two');
  await call(one.secret, 'POST', '/groups/join', { code: group.invite });
  await call(two.secret, 'POST', '/groups/join', { code: group.invite });
  const posted = await call(owner.secret, 'POST', '/playlists', { name: 'For you', items: [song('one')], to: [one.id] }, group.id);
  assert.equal((await call(one.secret, 'GET', '/playlists', undefined, group.id)).body.playlists.length, 1);
  assert.equal((await call(two.secret, 'GET', '/playlists', undefined, group.id)).body.playlists.length, 0);
  assert.equal((await call(two.secret, 'GET', `/playlists/${posted.body.id}`, undefined, group.id)).status, 404);
});

// Parties.
async function party() {
  const w = world();
  const host = await w.signIn('host');
  const { body: group } = await w.call(host.secret, 'POST', '/groups', { name: 'Studio' });
  const guest = await w.signIn('guest');
  await w.call(guest.secret, 'POST', '/groups/join', { code: group.invite });
  const { body: { channel } } = await w.call(host.secret, 'POST', '/live/channels', { name: 'Friday', item: song('first'), position_ms: 0, playing: true, locked: true }, group.id);
  await w.call(guest.secret, 'POST', `/live/channels/${channel.id}/join`, {}, group.id);
  const at = (verb: string) => `/live/channels/${channel.id}/${verb}`;
  return { ...w, host, guest, group, channel, at };
}

test('in a party a guest cannot seek, pause, skip or cut in, and the host can', async () => {
  const { call, host, guest, group, at } = await party();
  assert.equal((await call(guest.secret, 'POST', at('playback'), { playing: true, position_ms: 90000 }, group.id)).status, 403);
  assert.equal((await call(guest.secret, 'POST', at('playback'), { playing: false, position_ms: 0 }, group.id)).status, 403);
  assert.equal((await call(guest.secret, 'POST', at('next'), {}, group.id)).status, 403);
  assert.equal((await call(guest.secret, 'POST', at('queue'), { item: song('x'), where: 'now' }, group.id)).status, 403);
  assert.equal((await call(host.secret, 'POST', at('playback'), { playing: true, position_ms: 90000 }, group.id)).status, 200);
});

test('a guest may add to, reorder and take from the queue', async () => {
  const { call, guest, group, at } = await party();
  const one = await call(guest.secret, 'POST', at('queue'), { item: song('one') }, group.id);
  assert.equal(one.status, 201);
  await call(guest.secret, 'POST', at('queue'), { item: song('two') }, group.id);
  const entries = (await call(guest.secret, 'GET', '/live', undefined, group.id)).body.channels[0].queue;
  const moved = await call(guest.secret, 'POST', at(`queue/${entries[1].id}/move`), { index: 0 }, group.id);
  assert.deepEqual(moved.body.channel.queue.map((e: any) => e.item.title), ['two', 'one']);
  assert.equal((await call(guest.secret, 'DELETE', at(`queue/${entries[1].id}`), undefined, group.id)).status, 200);
});

test('a guest can only report that a song really ended', async () => {
  const { call, guest, group, at, advance, channel } = await party();
  const current = channel.current.id;
  assert.equal((await call(guest.secret, 'POST', at('next'), { from: current }, group.id)).status, 403);
  advance(181000);
  assert.equal((await call(guest.secret, 'POST', at('next'), { from: current }, group.id)).status, 200);
});

test('a guest can ask for a skip, the host sees it, and moving on clears it', async () => {
  const { call, host, guest, group, at } = await party();
  const asked = await call(guest.secret, 'PUT', at('skip-request'), undefined, group.id);
  assert.deepEqual(asked.body.channel.skip_requests, [guest.id]);
  const moved = await call(host.secret, 'POST', at('next'), {}, group.id);
  assert.deepEqual(moved.body.channel.skip_requests, []);
});

test('the host can share the controls, hand over, and end the party for everyone', async () => {
  const { call, host, guest, group, at } = await party();
  await call(host.secret, 'PUT', at('controllers'), { user_ids: [guest.id] }, group.id);
  assert.equal((await call(guest.secret, 'POST', at('playback'), { playing: true, position_ms: 5000 }, group.id)).status, 200);
  assert.equal((await call(guest.secret, 'POST', at('end'), {}, group.id)).status, 403, 'sharing the controls is not handing over the party');
  assert.equal((await call(guest.secret, 'POST', at('host'), { user_id: guest.id }, group.id)).status, 403);
  const handed = await call(host.secret, 'POST', at('host'), { user_id: guest.id }, group.id);
  assert.equal(handed.body.channel.host_id, guest.id);
  assert.equal((await call(guest.secret, 'POST', at('end'), {}, group.id)).status, 200);
  assert.deepEqual((await call(host.secret, 'GET', '/live', undefined, group.id)).body.channels, []);
});

test('a party whose host leaves goes to whoever has been there longest', async () => {
  const { call, host, guest, group, at } = await party();
  await call(host.secret, 'POST', at('leave'), {}, group.id);
  const { body } = await call(guest.secret, 'GET', '/live', undefined, group.id);
  assert.equal(body.channels[0].host_id, guest.id);
  assert.equal((await call(guest.secret, 'POST', at('playback'), { playing: true, position_ms: 1000 }, group.id)).status, 200);
});

test('a background playlist plays when the queue runs out, after anything queued, and loops', async () => {
  const { call, host, guest, group, at } = await party();
  const set = await call(guest.secret, 'PUT', at('background'), { name: 'Mine', items: [song('a'), song('b')] }, group.id);
  assert.equal(set.body.channel.background.total, 2);
  await call(guest.secret, 'POST', at('queue'), { item: song('queued') }, group.id);
  const titles = [];
  for (let i = 0; i < 4; i++) titles.push((await call(host.secret, 'POST', at('next'), {}, group.id)).body.channel.current.item.title);
  assert.deepEqual(titles, ['queued', 'a', 'b', 'a']);
  const live = (await call(host.secret, 'GET', '/live', undefined, group.id)).body.channels[0];
  assert.equal(live.background.upcoming[0].title, 'b');
  assert(JSON.stringify(live).length < 4000, 'the list itself stays out of what is polled');
});

test('a guest may not replace someone else\'s background playlist', async () => {
  const { call, host, guest, group, at } = await party();
  await call(host.secret, 'PUT', at('background'), { name: 'Host', items: [song('a')] }, group.id);
  assert.equal((await call(guest.secret, 'PUT', at('background'), { name: 'Mine', items: [song('b')] }, group.id)).status, 403);
  assert.equal((await call(guest.secret, 'DELETE', at('background'), undefined, group.id)).status, 403);
});

test('rights reach parties: no hosting, no queueing', async () => {
  const { call, host, guest, group, at, channel } = await party();
  await call(host.secret, 'PATCH', `/groups/${group.id}/members/${guest.id}`, { rights: { host: false, queue: false } });
  // Who belongs is remembered per instance for a minute; a change made through this one clears it.
  assert.equal((await call(guest.secret, 'POST', at('queue'), { item: song('x') }, group.id)).status, 403);
  assert.equal((await call(guest.secret, 'POST', '/live/channels', { name: 'Mine', locked: true }, group.id)).status, 403);
  assert.ok(channel.id);
});

test('a channel from an older client stays open to all of its members', async () => {
  const { call, legacy } = world();
  const a = await legacy('A'); const b = await legacy('B');
  const { body: { channel } } = await call(a.secret, 'POST', '/live/channels', { name: 'Old', item: song('s') });
  await call(b.secret, 'POST', `/live/channels/${channel.id}/join`, {});
  assert.equal((await call(b.secret, 'POST', `/live/channels/${channel.id}/playback`, { playing: false, position_ms: 10 })).status, 200);
  assert.equal((await call(b.secret, 'POST', `/live/channels/${channel.id}/next`, {})).status, 200);
});

test('groups keep their parties apart', async () => {
  const { call, host, group, signIn } = await party();
  const other = await signIn('other');
  const { body: second } = await call(other.secret, 'POST', '/groups', { name: 'Other' });
  assert.deepEqual((await call(other.secret, 'GET', '/live', undefined, second.id)).body.channels, []);
  assert.equal((await call(host.secret, 'GET', '/live', undefined, group.id)).body.channels.length, 1);
});


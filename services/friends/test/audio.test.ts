import assert from 'node:assert/strict';
import test from 'node:test';
import { MAX_SONG_BYTES, MAX_SONG_MS, MAX_SONGS, MONTH_BYTES, RESERVED_MS, STORE_BYTES, UPLOADS_PER_HOUR, audioRoute, makeRoom, type AudioStore } from '../src/audio.js';
import type { SharedAudio, State, User } from '../src/storage.js';
import { MemoryStorage } from '../src/storage.js';

const GROUP = 'group-1';
const ana: User = { user_id: 'ana', display_name: 'Ana', secret_hash: 'x' };
const ben: User = { user_id: 'ben', display_name: 'Ben', secret_hash: 'y' };

function song(id: string, size: number, created_at: number, extra: Partial<SharedAudio> = {}): SharedAudio {
  return { id, group_id: GROUP, by: 'ana', title: id, artists: [], duration_ms: 1000, size, ext: 'mp3', created_at, url: `https://blob/${id}`, ...extra };
}

function fakeStore() {
  const files = new Map<string, number>();
  const removed: string[] = [];
  const tokens: { pathname: string; bytes: number; contentType: string }[] = [];
  const store: AudioStore = {
    async uploadToken(pathname, bytes, contentType) { tokens.push({ pathname, bytes, contentType }); return `token-${pathname}`; },
    async find(pathname) { const size = files.get(pathname); return size === undefined ? null : { url: `https://blob/${pathname}`, size }; },
    async remove(pathnames) { removed.push(...pathnames); },
  };
  return { store, files, removed, tokens };
}

function harness(initial: Partial<State> = {}, clock = () => 1_000_000) {
  const state: State = { revision: 0, users: [ana, ben], posts: [], ...initial };
  const fake = fakeStore();
  const storage = new MemoryStorage();
  const send = (user: User, method: string, path: string, input?: unknown, overrides: { mayShare?: boolean; manages?: boolean; noStore?: boolean } = {}) => {
    const request = new Request(`http://local/api${path}`, {
      method,
      ...(input === undefined ? {} : { body: JSON.stringify(input), headers: { 'content-type': 'application/json' } }),
    });
    return audioRoute({
      request, path, user, groupId: GROUP, mayShare: overrides.mayShare ?? true, manages: overrides.manages ?? false,
      state: structuredClone(state), storage, store: overrides.noStore ? null : fake.store, clock, signal: new AbortController().signal,
      change: async update => update(state, user),
    });
  };
  return { state, send, ...fake };
}

const upload = { title: 'Midnight', artists: ['Ana'], duration_ms: 180_000, size: 8_000_000, ext: 'mp3' };

/** The failure a route threw, as the service would answer it. */
async function status(attempt: Promise<Response | null>): Promise<number> {
  try { return (await attempt)!.status; } catch (error) { return (error as { status: number }).status; }
}

test('a song is reserved, uploaded, finished, listed and played', async () => {
  const { send, files, tokens, state } = harness();
  const reserved = await send(ana, 'POST', '/audio', upload);
  assert.equal(reserved!.status, 201);
  const { id, pathname, token, content_type } = await reserved!.json();
  assert.equal(content_type, 'audio/mpeg');
  assert.equal(token, `token-${pathname}`);
  // The store itself refuses anything larger than was declared.
  assert.equal(tokens[0].bytes, upload.size);

  const hidden = await (await send(ben, 'GET', '/audio'))!.json();
  assert.equal(hidden.songs.length, 0, 'an unfinished upload is not listed');
  assert.equal(await status(send(ana, 'POST', `/audio/${id}/complete`, {})), 409, 'nothing arrived yet');

  files.set(pathname, 7_500_000);
  assert.equal(await status(send(ben, 'POST', `/audio/${id}/complete`, {})), 403);
  assert.equal((await send(ana, 'POST', `/audio/${id}/complete`, {}))!.status, 200);
  assert.equal(state.audio![0].size, 7_500_000, 'what is counted is what arrived');

  const listed = await (await send(ben, 'GET', '/audio'))!.json();
  assert.equal(listed.songs[0].by_name, 'Ana');
  assert.equal(listed.songs[0].url, undefined, 'the file address comes only through play');

  const played = await (await send(ben, 'POST', `/audio/${id}/play`, {}))!.json();
  assert.match(played.url, /^https:\/\/blob\//);
  assert.equal(state.audio_month!.bytes, 7_500_000);
});

test('what cannot be shared is refused before anything is stored', async () => {
  const { send, state, tokens } = harness();
  assert.equal(await status(send(ana, 'POST', '/audio', { ...upload, duration_ms: MAX_SONG_MS + 1 })), 413, 'an hour-long song');
  assert.equal(await status(send(ana, 'POST', '/audio', { ...upload, size: MAX_SONG_BYTES + 1 })), 413, 'a huge file');
  assert.equal(await status(send(ana, 'POST', '/audio', { ...upload, ext: 'exe' })), 415);
  assert.equal(await status(send(ana, 'POST', '/audio', { ...upload, ext: 'constructor' })), 415);
  assert.equal(await status(send(ana, 'POST', '/audio', upload, { mayShare: false })), 403);
  assert.equal(state.audio, undefined);
  assert.equal(tokens.length, 0);
});

test('uploads are limited by the hour, not by a personal allowance', async () => {
  const { send } = harness();
  for (let i = 0; i < UPLOADS_PER_HOUR; i++) assert.equal(await status(send(ana, 'POST', '/audio', upload)), 201);
  assert.equal(await status(send(ana, 'POST', '/audio', upload)), 429, 'one person dumping files is stopped');
  assert.equal(await status(send(ben, 'POST', '/audio', upload)), 201, 'without stopping anyone else');
});

test('the same file is not stored twice, whoever sends it', async () => {
  const { send, tokens } = harness({ audio: [song('kept', upload.size, 1, { title: upload.title })] });
  assert.equal(await status(send(ben, 'POST', '/audio', upload)), 409);
  assert.equal(tokens.length, 0, 'nothing was offered for upload');
  assert.equal(await status(send(ben, 'POST', '/audio', { ...upload, title: 'Another' })), 201);
  assert.equal(await status(send(ben, 'POST', '/audio', { ...upload, size: upload.size + 1 })), 201);
});

test('the oldest songs make room, and a full store never overflows', async () => {
  const size = 30 * 1024 * 1024;
  const songs = Array.from({ length: 30 }, (_, i) => song(`s${String(i).padStart(2, '0')}`, size, i));
  const { send, state, removed } = harness({ audio: songs });
  assert.equal(await status(send(ben, 'POST', '/audio', { ...upload, size })), 201);
  const total = state.audio!.reduce((sum, row) => sum + row.size, 0);
  assert.ok(total <= STORE_BYTES, `${total} fits`);
  assert.deepEqual(removed, ['audio/s00.mp3'], 'only the oldest went');
  assert.ok(!state.audio!.some(row => row.id === 's00'));
});

test('the song count has a ceiling too', () => {
  const songs = Array.from({ length: MAX_SONGS }, (_, i) => song(`s${i}`, 1, i));
  const state: State = { revision: 0, users: [], posts: [], audio: songs };
  const gone = makeRoom(state, 1, 10_000);
  assert.deepEqual(gone.map(row => row.id), ['s0']);
});

test('an upload that never finished stops holding room', () => {
  const now = 10_000_000;
  const state: State = {
    revision: 0, users: [], posts: [],
    audio: [
      song('abandoned', STORE_BYTES, now - RESERVED_MS - 1, { url: null }),
      song('in-flight', 10, now - 1000, { url: null }),
    ],
  };
  const gone = makeRoom(state, 1000, now);
  assert.deepEqual(gone.map(row => row.id), ['abandoned']);
  assert.deepEqual(state.audio!.map(row => row.id), ['in-flight']);
});

test('downloads stop before the month runs out, and come back next month', async () => {
  let now = Date.UTC(2026, 10, 15);
  const audio = [song('11111111-1111-1111-1111-111111111111', 1000, 1)];
  const { send, state } = harness({ audio, audio_month: { month: '2026-11', bytes: MONTH_BYTES - 500 } }, () => now);
  const path = `/audio/${audio[0].id}/play`;
  assert.equal(await status(send(ben, 'POST', path, {})), 429);
  now = Date.UTC(2026, 11, 1);
  assert.equal(await status(send(ben, 'POST', path, {})), 200);
  assert.deepEqual(state.audio_month, { month: '2026-12', bytes: 1000 });
});

test('only the uploader or someone who runs the group can delete', async () => {
  const id = '22222222-2222-2222-2222-222222222222';
  const { send, state, removed } = harness({ audio: [song(id, 1000, 1)] });
  assert.equal(await status(send(ben, 'DELETE', `/audio/${id}`)), 403);
  assert.equal(await status(send(ben, 'DELETE', `/audio/${id}`, undefined, { manages: true })), 200);
  assert.equal(state.audio!.length, 0);
  assert.deepEqual(removed, [`audio/${id}.mp3`]);
});

test('another group cannot see or play a song', async () => {
  const id = '33333333-3333-3333-3333-333333333333';
  const { send } = harness({ audio: [song(id, 1000, 1, { group_id: 'other' })] });
  assert.equal((await (await send(ben, 'GET', '/audio'))!.json()).songs.length, 0);
  assert.equal(await status(send(ben, 'POST', `/audio/${id}/play`, {})), 404);
});

test('a service without a store says so', async () => {
  const { send } = harness();
  assert.equal(await status(send(ben, 'GET', '/audio', undefined, { noStore: true })), 503);
});

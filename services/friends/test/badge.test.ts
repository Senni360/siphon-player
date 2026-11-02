import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { tintGif } from '../src/badge.js';
import { createService } from '../src/service.js';
import { MemoryStorage } from '../src/storage.js';

const source = readFileSync(new URL('../assets/encore-badge.gif', import.meta.url));
const orange = [255, 85, 0] as const;
const globalTable = (gif: Uint8Array) => Array.from({ length: 256 }, (_, entry) => [...gif.subarray(13 + entry * 3, 16 + entry * 3)]);

test('the service serves the same picture the published apps link on GitHub', () => {
  assert.deepEqual(source, readFileSync(new URL('../../../packaging/icons/encore-badge.gif', import.meta.url)));
});

test('a lit badge changes colours and nothing about its frames', () => {
  const lit = tintGif(source, orange);
  assert.equal(lit.length, source.length);
  // Everything before the first colour table is the same, the size included.
  assert.deepEqual(lit.subarray(0, 13), new Uint8Array(source.subarray(0, 13)));
  // Every frame's own table took the colour, not only the first.
  const last = lit.findLastIndex((byte, at) => byte !== source[at]);
  assert.ok(last > source.length * 0.9, `the last colour changed at ${last} of ${source.length}`);
  const before = globalTable(source);
  const after = globalTable(lit);
  const brightest = before.reduce((best, entry, at) => Math.max(...entry) > Math.max(...before[best]) ? at : best, 0);
  after[brightest].forEach((channel, at) => assert.ok(Math.abs(channel - orange[at]) <= 3, `${after[brightest]}`));
  const plate = before.findIndex(entry => Math.max(...entry) / 255 < 0.2);
  assert.deepEqual(after[plate], before[plate], 'the plate stays dark');
});

test('white rings stay as they are drawn', () => {
  const white = globalTable(tintGif(source, [255, 255, 255]));
  globalTable(source).forEach((entry, at) => entry.forEach((channel, rgb) => assert.ok(Math.abs(channel - white[at][rgb]) <= 12)));
});

test('the badge needs no sign-in, is kept for good, and takes only a colour', async () => {
  const service = createService(new MemoryStorage(), 'a-long-test-invite');
  const answer = await service(new Request('https://test/api/badge/f50.gif'));
  assert.equal(answer.status, 200);
  assert.equal(answer.headers.get('content-type'), 'image/gif');
  assert.match(answer.headers.get('cache-control') ?? '', /immutable/);
  const body = new Uint8Array(await answer.arrayBuffer());
  assert.deepEqual(body, tintGif(source, orange));
  for (const wrong of ['ff5500', 'f5', 'F50', 'xyz']) {
    assert.notEqual((await service(new Request(`https://test/api/badge/${wrong}.gif`))).status, 200, wrong);
  }
});

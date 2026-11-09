import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

// The phone's own sign-out decision, lifted out of the module that owns it
// between two markers so it can be asked about here without a DOM.
const app = readFileSync(new URL('../assets/web/js/auth.js', import.meta.url), 'utf8');
const begin = app.indexOf('// [signout-begin]');
const end = app.indexOf('// [signout-end]');
assert.ok(begin >= 0 && end > begin, 'auth.js marks its sign-out decision for these tests');
const removedDevice: (answer: Answer, attached: boolean, probe: () => Answer) => Promise<boolean> =
  vm.runInNewContext(`${app.slice(begin, end)}\nremovedDevice`, vm.createContext({}));

// A fetch answer, or something a proxy might answer instead.
type Answer = { status: number; json: () => Promise<unknown> };
const answer = (status: number, value: unknown, broken = false): Answer => ({
  status,
  json: () => (broken ? Promise.reject(new Error('not json')) : Promise.resolve(value)),
});
const refused = (): Answer => answer(401, { error: 'Sign in to the group' });
const probeOf = (next: () => Answer) => async (): Promise<Answer> => next();
const refusedAgain = () => refused();
const workingAgain = () => answer(200, { user_id: 'u' });
const offlineAgain = () => {
  throw new Error('no network');
};

test('a transient failure never signs the phone out', async () => {
  assert.equal(await removedDevice(answer(503, { error: 'Service unavailable' }, true), true, refusedAgain), false, '5xx is not a refusal');
  assert.equal(await removedDevice(answer(429, { error: 'Too many requests' }), true, refusedAgain), false, '429 is not a refusal');
  assert.equal(await removedDevice(answer(401, { error: 'Not json' }, true), true, refusedAgain), false, 'a proxy that cannot answer does not');
  assert.equal(await removedDevice(answer(401, '<html>Sign in</html>', true), true, refusedAgain), false, 'a login page is not the service');
  assert.equal(await removedDevice(refused(), true, offlineAgain), false, 'the confirming ask going nowhere');
  assert.equal(await removedDevice(refused(), true, workingAgain), false, 'the service still knows this device');
  assert.equal(await removedDevice(refused(), false, refusedAgain), false, 'nothing was sent to refuse: no secret attached');
  assert.equal(await removedDevice(answer(403, { error: 'You are not in this group' }), true, refusedAgain), false, 'not even a refusal');
});

test('only a fresh, repeated refusal about this very secret signs the phone out', async () => {
  assert.equal(await removedDevice(refused(), true, refusedAgain), true, 'the service twice over');
});

test('a 401 that speaks for someone else never does', async () => {
  // What POST /auth/spotify answers when Spotify refuses its token, while the
  // phone's own device secret is still perfectly fine.
  assert.equal(await removedDevice(answer(401, { error: 'Spotify did not accept this sign-in' }), true, refusedAgain), false);
});

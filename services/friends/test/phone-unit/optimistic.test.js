import assert from 'node:assert/strict';
import test from 'node:test';
import { createOptimistic } from '../../assets/web/js/optimistic.js';

/** A clock and timer queue the test moves by hand. */
function clockwork() {
  let at = 0;
  let next = 0;
  const timers = new Map();
  return {
    now: () => at,
    schedule(fn, ms) {
      next += 1;
      timers.set(next, { fn, due: at + ms });
      return next;
    },
    cancel(handle) {
      timers.delete(handle);
    },
    advance(ms) {
      at += ms;
      for (const [handle, timer] of [...timers]) {
        if (timer.due <= at) {
          timers.delete(handle);
          timer.fn();
        }
      }
    },
  };
}

const setup = (options = {}) => {
  const clock = clockwork();
  const expiring = [];
  const view = createOptimistic({
    now: clock.now,
    schedule: clock.schedule,
    cancel: clock.cancel,
    timeout: 4000,
    onExpire: (field, value, token) => expiring.push({ field, value, token }),
    ...options,
  });
  return { clock, view, expiring };
};

test('a press shows its value at once and holds it', () => {
  const { view } = setup();
  const token = view.press('shuffle', true);

  assert.equal(view.shown('shuffle', false), true);
  assert.equal(view.pending('shuffle'), true);
  assert.equal(typeof token, 'number');
});

test('an answer that agrees settles the field', () => {
  const { view } = setup();
  const token = view.press('shuffle', true);
  assert.equal(view.answer('shuffle', token, true), true);
  assert.equal(view.pending('shuffle'), false);
  assert.equal(view.shown('shuffle', true), true);
});

test('an answer that disagrees is remembered but does not flicker the control', () => {
  const { view } = setup();
  const token = view.press('shuffle', true);

  view.answer('shuffle', token, false);

  assert.equal(view.shown('shuffle', false), true);
  assert.equal(view.pending('shuffle'), true);
});

test('an answer from before the latest action is stale', () => {
  const { view } = setup();
  const first = view.press('shuffle', true);
  const second = view.press('shuffle', false);

  assert.equal(view.answer('shuffle', first, true), false);
  assert.equal(view.shown('shuffle', true), false);
  assert.equal(view.answer('shuffle', second, false), true);
  assert.equal(view.pending('shuffle'), false);
});

test('after the hold runs out it asks again and keeps the shown value', () => {
  const { view, clock, expiring } = setup();
  view.press('shuffle', true);
  const before = view.shown('shuffle', false);

  clock.advance(4000);

  assert.equal(expiring.length, 1);
  assert.equal(expiring[0].field, 'shuffle');
  assert.equal(expiring[0].value, true);
  assert.equal(view.shown('shuffle', false), before);
  assert.equal(view.pending('shuffle'), true);
});

test('the answer to the re-ask is taken, and the one still in flight is stale', () => {
  const { view, clock, expiring } = setup();
  const first = view.press('shuffle', true);
  clock.advance(4000);

  const token = expiring[0].token;
  assert.equal(view.answer('shuffle', first, false), false);  // from before the re-ask
  assert.equal(view.answer('shuffle', token, false), true);   // the fresh answer
  assert.equal(view.shown('shuffle', false), false);
  assert.equal(view.pending('shuffle'), false);
});

test('an answer to the re-ask that agrees keeps the control where it is', () => {
  const { view, clock, expiring } = setup();
  view.press('like', true);
  clock.advance(4000);

  view.answer('like', expiring[0].token, true);

  assert.equal(view.pending('like'), false);
  assert.equal(view.shown('like'), true);
});

test('a field that is already shown by the backend can be used without a press', () => {
  const { view } = setup();
  assert.equal(view.shown('repeat', 'all'), 'all');
  assert.equal(view.shown('repeat'), 'all');
  assert.equal(view.pending('repeat'), false);
});

test('begin can start an ask whose shown value is set elsewhere', () => {
  const { view } = setup();
  const token = view.begin('seek');
  view.shown('seek', 12000);
  assert.equal(view.answer('seek', token, 12500), true);
  assert.equal(view.shown('seek'), 12500);
});

test('forget clears the hold and its timer', () => {
  const { view, clock, expiring } = setup();
  view.press('volume', 70);
  view.forget('volume');
  clock.advance(4000);
  assert.equal(expiring.length, 0);
  assert.equal(view.pending('volume'), false);
});

test('two controls do not interfere', () => {
  const { view } = setup();
  const shuffle = view.press('shuffle', true);
  const like = view.press('like', true);

  assert.equal(view.answer('shuffle', shuffle, false), true);
  assert.equal(view.shown('shuffle', false), true);
  assert.equal(view.shown('like', false), true);
  assert.equal(view.answer('like', like, true), true);
  assert.equal(view.shown('like'), true);
});

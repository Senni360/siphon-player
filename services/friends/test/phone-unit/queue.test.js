import assert from 'node:assert/strict';
import test from 'node:test';
import {
  createQueue, current, emptyQueue, next, previous, jump, addNext, addLast, remove, move,
  setRepeat, setShuffle, restore, snapshot, upcoming,
} from '../../assets/web/js/queue.js';

const A = { id: 'a' };
const B = { id: 'b' };
const C = { id: 'c' };
const D = { id: 'd' };
const X = { id: 'x' };

const queueOf = (list, index = 0, extra = {}) => ({ ...emptyQueue(), list, index, ...extra });
const id = entry => entry?.id;

test('current and upcoming read the play order', () => {
  const queue = queueOf([A, B, C], 1);
  assert.equal(current(queue), B);
  assert.deepEqual(upcoming(queue).map(id), ['c']);
  assert.equal(current(emptyQueue()), undefined);
  assert.deepEqual(upcoming(emptyQueue()), []);
});

test('next moves along and remembers what played', () => {
  const first = queueOf([A, B, C], 0);
  const second = next(first);
  assert.equal(second.moved, true);
  assert.equal(id(second.entry), 'b');
  assert.equal(second.state.index, 1);
  assert.deepEqual(second.state.history.map(id), ['a']);

  const third = next(second.state);
  assert.equal(third.state.index, 2);
  assert.deepEqual(third.state.history.map(id), ['a', 'b']);
});

test('the end is finished when Repeat is off', () => {
  const end = queueOf([A, B], 1);
  const result = next(end);

  assert.equal(result.moved, false);
  assert.equal(result.finished, true);
  assert.equal(result.state.index, 1);
  assert.deepEqual(result.state.history, []);
});

test('Repeat all goes back to the top', () => {
  const end = queueOf([A, B, C], 2, { repeat: 'all' });
  const result = next(end);

  assert.equal(result.moved, true);
  assert.equal(result.wrapped, true);
  assert.equal(result.state.index, 0);
  assert.equal(id(result.entry), 'a');
  assert.deepEqual(result.state.history.map(id), ['c']);
});

test('Repeat one keeps the same song when it ends, but not on a hand skip', () => {
  const queue = queueOf([A, B, C], 1, { repeat: 'one' });

  const ended = next(queue);
  assert.equal(ended.moved, true);
  assert.equal(ended.repeated, true);
  assert.equal(ended.state.index, 1);
  assert.equal(id(ended.entry), 'b');
  assert.deepEqual(ended.state.history, []);

  const skipped = next(queue, { ended: false });
  assert.equal(skipped.moved, true);
  assert.equal(skipped.state.index, 2);
  assert.equal(id(skipped.entry), 'c');
});

test('previous uses the history, then the order, then restarts', () => {
  const third = next(next(queueOf([A, B, C], 0)).state).state;
  assert.equal(third.index, 2);

  const back = previous(third);
  assert.equal(back.moved, true);
  assert.equal(back.state.index, 1);
  assert.equal(id(back.entry), 'b');

  const start = previous(queueOf([A, B, C], 1));
  assert.equal(start.moved, true);
  assert.equal(start.state.index, 0);

  const first = previous(queueOf([A, B, C], 0));
  assert.equal(first.moved, false);
  assert.equal(first.restart, true);
  assert.equal(first.state.index, 0);
});

test('jump skips rows, and previous returns to where the listener came from', () => {
  const jumped = jump(queueOf([A, B, C, D], 0), 2);
  assert.equal(jumped.moved, true);
  assert.equal(jumped.state.index, 2);
  assert.deepEqual(jumped.state.history.map(id), ['a']);

  const back = previous(jumped.state);
  assert.equal(back.state.index, 0);
  assert.equal(id(back.entry), 'a');
});

test('jump to the playing row restarts it', () => {
  const result = jump(queueOf([A, B], 1), 1);
  assert.equal(result.moved, false);
  assert.equal(result.restart, true);
  assert.equal(result.state.index, 1);
});

test('add next puts songs right after the current one; add last puts them at the end', () => {
  const nexted = addNext(queueOf([A, B, C], 1), [X]);
  assert.deepEqual(nexted.state.list.map(id), ['a', 'b', 'x', 'c']);

  const lasted = addLast(nexted.state, [D, X]);
  assert.deepEqual(lasted.state.list.map(id), ['a', 'b', 'x', 'c', 'd', 'x']);
  assert.equal(lasted.state.index, 1);
});

test('adding to an empty queue starts it', () => {
  const started = addNext(emptyQueue(), [A, B]);
  assert.deepEqual(started.state.list.map(id), ['a', 'b']);
  assert.equal(started.state.index, 0);

  const appended = addLast(emptyQueue(), [A]);
  assert.equal(appended.state.index, 0);
});

test('adding to a finished queue starts at the newly queued song', () => {
  const finished = queueOf([A, B], -1);
  const nexted = addNext(finished, [X]);
  assert.equal(id(current(nexted.state)), 'x');
  assert.equal(nexted.state.index, 2);

  const lasted = addLast(finished, [C, D]);
  assert.equal(id(current(lasted.state)), 'c');
  assert.equal(lasted.state.index, 2);
});

test('removing a row above or below keeps the playing row', () => {
  const below = remove(queueOf([A, B, C], 1), 2);
  assert.equal(id(below.removed), 'c');
  assert.equal(below.state.index, 1);
  assert.equal(id(current(below.state)), 'b');

  const above = remove(queueOf([A, B, C], 2), 0);
  assert.equal(above.state.index, 1);
  assert.equal(id(current(above.state)), 'c');
});

test('removing the playing row makes the next one current', () => {
  const result = remove(queueOf([A, B, C], 1), 1);
  assert.deepEqual(result.state.list.map(id), ['a', 'c']);
  assert.equal(result.state.index, 1);
  assert.equal(id(current(result.state)), 'c');
});

test('removing the playing last row leaves a finished queue', () => {
  const result = remove(queueOf([A, B], 1), 1);
  assert.deepEqual(result.state.list.map(id), ['a']);
  assert.equal(result.state.index, -1);
  assert.equal(current(result.state), undefined);

  const after = next(result.state);
  assert.equal(after.finished, true);
  assert.equal(after.state.index, -1);
});

test('moving a row keeps the same song current', () => {
  const moved = move(queueOf([A, B, C], 1), 0, 2);
  assert.deepEqual(moved.state.list.map(id), ['b', 'c', 'a']);
  assert.equal(moved.state.index, 0);
  assert.equal(id(current(moved.state)), 'b');

  const itself = move(queueOf([A, B, C], 1), 1, 2);
  assert.deepEqual(itself.state.list.map(id), ['a', 'c', 'b']);
  assert.equal(itself.state.index, 2);
  assert.equal(id(current(itself.state)), 'b');
});

test('shuffle keeps the playing song first and unshuffle brings back the order', () => {
  const queue = queueOf([A, B, C, D], 1);
  const mixed = setShuffle(queue, true, { random: () => 0 });

  assert.equal(id(mixed.list[0]), 'b');
  assert.equal(mixed.index, 0);
  assert.equal(mixed.shuffle, true);
  assert.deepEqual(mixed.original.map(id), ['a', 'b', 'c', 'd']);
  assert.deepEqual(
    [...mixed.list].map(id).sort(),
    ['a', 'b', 'c', 'd'],
  );

  const played = next(mixed, { ended: false });
  const backToOrder = setShuffle(played.state, false);

  assert.deepEqual(backToOrder.list.map(id), ['a', 'b', 'c', 'd']);
  assert.equal(backToOrder.shuffle, false);
  assert.equal(backToOrder.original, null);
  assert.equal(id(current(backToOrder)), id(current(played.state)));
  assert.equal(backToOrder.index, 2);  // c was playing, and c is third in the original order
});

test('toggling shuffle with no current row keeps a finished queue finished', () => {
  const finished = queueOf([A, B], -1);
  const mixed = setShuffle(finished, true, { random: () => 0 });
  assert.equal(mixed.index, -1);

  const plain = setShuffle(mixed, false);
  assert.deepEqual(plain.list.map(id), ['a', 'b']);
  assert.equal(plain.index, -1);
});

test('a saved queue comes back the same, and is repaired if it makes no sense', () => {
  const queue = queueOf([A, B, C], 2, { repeat: 'all', shuffle: false, history: [A, B] });
  const saved = JSON.parse(JSON.stringify(snapshot(queue)));
  const restored = restore(saved);

  assert.deepEqual(restored.list.map(id), ['a', 'b', 'c']);
  assert.equal(restored.index, 2);
  assert.equal(restored.repeat, 'all');
  assert.deepEqual(restored.history.map(id), ['a', 'b']);

  const repaired = restore({ list: [A, B], index: 99, repeat: 'sometimes', history: 'no' });
  assert.equal(repaired.index, 0);
  assert.equal(repaired.repeat, 'off');
  assert.deepEqual(repaired.history, []);

  const nothing = restore(null);
  assert.deepEqual(nothing, emptyQueue());
});

test('repeat modes are the three the app knows', () => {
  assert.equal(setRepeat(emptyQueue(), 'one').repeat, 'one');
  assert.throws(() => setRepeat(emptyQueue(), 'twice'), TypeError);
});

test('the factory keeps its own state and can be saved and restored', () => {
  const queue = createQueue([A, B, C]);
  assert.equal(id(queue.current), 'a');
  assert.deepEqual(queue.upcoming.map(id), ['b', 'c']);

  queue.next();
  queue.addNext([X]);
  queue.setRepeat('all');
  assert.equal(id(queue.current), 'b');
  assert.deepEqual(queue.list.map(id), ['a', 'b', 'x', 'c']);
  assert.equal(queue.repeat, 'all');

  const saved = queue.snapshot();
  const other = createQueue();
  other.restore(saved);
  assert.equal(id(other.current), 'b');
  assert.deepEqual(other.list.map(id), ['a', 'b', 'x', 'c']);

  assert.equal(queue.toggleShuffle({ random: () => 0 }), true);
  assert.equal(id(queue.list[0]), 'b');
  assert.equal(queue.toggleShuffle(), false);
  assert.deepEqual(queue.list.map(id), ['a', 'b', 'x', 'c']);
});

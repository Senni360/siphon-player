import assert from 'node:assert/strict';
import test from 'node:test';
import { createStore } from '../../assets/web/js/store.js';

const settle = () => new Promise(resolve => queueMicrotask(resolve));

test('get reads the state and a selector', () => {
  const store = createStore({ count: 2, name: 'Encore' });
  assert.deepEqual(store.get(), { count: 2, name: 'Encore' });
  assert.equal(store.get(state => state.count), 2);
  assert.equal(store.get('name'), 'Encore');
});

test('a subscriber hears only changes to its selected value', async () => {
  const store = createStore({ count: 0, other: 0 });
  const heard = [];
  store.subscribe(state => state.count, value => heard.push(value));

  store.set({ other: 1 });
  await settle();
  assert.deepEqual(heard, []);

  store.set({ count: 1 });
  await settle();
  assert.deepEqual(heard, [1]);

  store.set({ count: 1 });
  await settle();
  assert.deepEqual(heard, [1]);
});

test('changes in one turn are announced once', async () => {
  const store = createStore({ count: 0 });
  const heard = [];
  store.subscribe(state => state.count, value => heard.push(value));

  store.set({ count: 1 });
  store.set({ count: 2 });
  store.update(state => ({ ...state, count: 3 }));
  assert.deepEqual(heard, []);  // still in this turn

  await settle();
  assert.deepEqual(heard, [3]);
});

test('a subscriber only hears values after it subscribed', async () => {
  const store = createStore({ count: 0 });
  const heard = [];
  store.subscribe(state => state.count, value => heard.push(value));

  store.set({ count: 5 });
  await settle();
  assert.deepEqual(heard, [5]);
});

test('immediate hands over the current value', () => {
  const store = createStore({ count: 4 });
  const heard = [];
  store.subscribe('count', value => heard.push(value), { immediate: true });
  assert.deepEqual(heard, [4]);
});

test('unsubscribe stops notifications', async () => {
  const store = createStore({ count: 0 });
  const heard = [];
  const stop = store.subscribe(state => state.count, value => heard.push(value));

  store.set({ count: 1 });
  await settle();
  stop();
  store.set({ count: 2 });
  await settle();

  assert.deepEqual(heard, [1]);
});

test('a function patch sees the state it is changing', async () => {
  const store = createStore({ count: 1 });
  store.set(state => ({ count: state.count + 1 }));
  assert.equal(store.get('count'), 2);

  store.update(state => ({ ...state, name: 'Encore' }));
  assert.deepEqual(store.get(), { count: 2, name: 'Encore' });
});

test('a selector may use its own equality', async () => {
  const store = createStore({ list: [1, 2] });
  let calls = 0;
  store.subscribe(
    state => state.list.map(number => number * 2),
    () => { calls += 1; },
    { equal: (a, b) => a.length === b.length && a.every((value, at) => value === b[at]) },
  );

  store.set({ list: [1, 2] });
  await settle();
  assert.equal(calls, 0);

  store.set({ list: [1, 2, 3] });
  await settle();
  assert.equal(calls, 1);
});

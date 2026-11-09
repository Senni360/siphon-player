import assert from 'node:assert/strict';
import test from 'node:test';
import { backoff, createConnection, EVENTS, STATES } from '../../assets/web/js/connection.js';

const fresh = (options = {}) => {
  const seen = [];
  let at = 0;
  const connection = createConnection({
    now: () => at,
    random: () => 0.5,
    onState: (state, previous, when) => seen.push({ state, previous, when }),
    ...options,
  });
  return { connection, seen, tick: ms => { at += ms; }, at: () => at };
};

test('the seven states are the whole vocabulary', () => {
  assert.deepEqual([...STATES], [
    'live', 'slow', 'reconnecting', 'asleep', 'offline', 'service_down', 'signed_out',
  ]);
  assert.ok(EVENTS.includes('computer-away'));
  assert.ok(EVENTS.includes('network-offline'));
});

test('a fresh page is live', () => {
  const { connection } = fresh();
  assert.equal(connection.state, 'live');
  assert.equal(connection.attempts, 0);
});

test('the service being reachable but the computer away is asleep', () => {
  const { connection } = fresh();
  assert.equal(connection.report('computer-away'), 'asleep');
  assert.equal(connection.report('computer-here'), 'live');
});

test('offline outranks everything but a removed device', () => {
  const { connection } = fresh();
  connection.report('computer-away');
  connection.report('service-down');
  assert.equal(connection.report('network-offline'), 'offline');

  connection.report('signed-out');
  assert.equal(connection.state, 'signed_out');
  assert.equal(connection.report('network-online'), 'signed_out');
});

test('being signed out sticks until a sign-in', () => {
  const { connection } = fresh();
  connection.report('signed-out');
  connection.report('answer');
  assert.equal(connection.state, 'signed_out');

  connection.report('signed-in');
  assert.equal(connection.state, 'reconnecting');
  connection.report('answer');
  assert.equal(connection.state, 'live');
});

test('a failed service is service_down, not reconnecting', () => {
  const { connection } = fresh();
  assert.equal(connection.report('service-down'), 'service_down');
  assert.equal(connection.report('server-error'), 'service_down');
  assert.equal(connection.report('service-ok'), 'live');
});

test('a lost stream reconnects, a reopened one is live again', () => {
  const { connection } = fresh();
  assert.equal(connection.report('lost'), 'reconnecting');
  assert.equal(connection.report('stream-open'), 'live');

  connection.report('timeout');
  assert.equal(connection.state, 'reconnecting');
  connection.report('answer');
  assert.equal(connection.state, 'live');
});

test('slow answers are their own state and clear on a fast one', () => {
  const { connection } = fresh();
  assert.equal(connection.report('slow'), 'slow');
  assert.equal(connection.report('fast'), 'live');

  connection.report('computer-away');
  connection.report('slow');
  assert.equal(connection.state, 'asleep');  // no computer is more useful than slow
});

test('listeners hear changes only, with the previous state', () => {
  const { connection, seen, tick } = fresh();
  connection.report('slow');
  connection.report('slow');
  tick(25);
  connection.report('fast');

  assert.deepEqual(seen.map(change => [change.previous, change.state]), [
    ['live', 'slow'],
    ['slow', 'live'],
  ]);
  assert.equal(connection.since, 25);
});

test('a subscriber can start with the current state and stop', () => {
  const { connection } = fresh();
  const heard = [];
  const stop = connection.subscribe(state => heard.push(state), { immediate: true });
  connection.report('slow');
  stop();
  connection.report('fast');

  assert.deepEqual(heard, ['live', 'slow']);
});

test('failures raise the retry wait and a good answer resets it', () => {
  const { connection } = fresh({ base: 1000, max: 8000, jitter: 0 });

  connection.report('lost');
  assert.equal(connection.attempts, 1);
  assert.equal(connection.retryDelay(), 1000);

  connection.report('timeout');
  assert.equal(connection.attempts, 2);
  assert.equal(connection.retryDelay(), 2000);

  connection.report('service-down');
  assert.equal(connection.retryDelay(), 4000);

  connection.report('answer');
  assert.equal(connection.attempts, 0);
  assert.equal(connection.retryDelay(), 1000);
});

test('backoff grows, stays jittered and is capped', () => {
  const low = backoff(0, { base: 1000, max: 30000, jitter: 0.25, random: () => 0 });
  const high = backoff(0, { base: 1000, max: 30000, jitter: 0.25, random: () => 1 });
  assert.equal(low, 750);
  assert.equal(high, 1000);

  assert.equal(backoff(2, { base: 1000, max: 30000, jitter: 0, random: () => 0.5 }), 4000);
  assert.equal(backoff(99, { base: 1000, max: 5000, jitter: 0.25, random: () => 1 }), 5000);
  assert.equal(backoff(99, { base: 1000, max: 5000, jitter: 0.25, random: () => 0 }), 3750);
});

test('an unknown report is a mistake, not silence', () => {
  const { connection } = fresh();
  assert.throws(() => connection.report('all-good'), TypeError);
});

test('reset returns to a fresh page', () => {
  const { connection } = fresh();
  connection.report('computer-away');
  connection.report('lost');
  connection.reset();

  assert.equal(connection.state, 'live');
  assert.equal(connection.attempts, 0);
});

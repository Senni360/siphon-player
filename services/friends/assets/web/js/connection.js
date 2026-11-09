// One connection state for the whole page.
//
// The phone sees several things at once: the browser's network, the group
// service, and the listener's computer behind it. Rather than let each screen
// invent its own "are we okay?", everything reports here and one of seven
// states comes out:
//
//   live         service answered and the computer is there
//   slow         service is reachable but answers are taking a while
//   reconnecting the stream or a request was lost; we are trying again
//   asleep       service is healthy but the computer is not connected
//   offline      the browser has no network
//   service_down the service itself is failing
//   signed_out   this device's secret was removed
//
// `backoff` and `retryDelay` give the wait before the next try, with jitter so
// a group of phones does not all come back at the same instant.

export const STATES = Object.freeze([
  'live', 'slow', 'reconnecting', 'asleep', 'offline', 'service_down', 'signed_out',
]);

/** The reported events the machine understands. */
export const EVENTS = Object.freeze([
  'answer', 'ok', 'fast', 'slow',
  'timeout', 'lost', 'stream-closed', 'stream-open', 'reconnecting',
  'service-down', 'server-error', 'service-ok',
  'computer-away', 'computer-here',
  'network-offline', 'network-online',
  'signed-out', 'signed-in',
]);

const clamp = (value, low, high) => Math.min(high, Math.max(low, value));

/**
 * An exponential wait with jitter. Attempt 0 is the first wait after a
 * failure. The result stays between `base * 2^attempt * (1 - jitter)` and that
 * raw value, and never above `max`.
 */
export function backoff(attempt, { base = 1000, max = 30000, jitter = 0.25, random = Math.random } = {}) {
  const raw = Math.min(max, base * 2 ** Math.max(0, attempt));
  const spread = raw * clamp(jitter, 0, 1);
  const low = Math.round(raw - spread);
  return Math.min(max, Math.max(0, low + Math.round(random() * spread)));
}

/**
 * @param {object} [options]
 * @param {(state: string, previous: string|null, at: number) => void} [options.onState]
 * @param {() => number} [options.now] Clock, for tests.
 * @param {() => number} [options.random] Jitter source, for tests.
 * @param {number} [options.base] First retry wait, in ms.
 * @param {number} [options.max] Longest retry wait, in ms.
 * @param {number} [options.jitter] Fraction of the wait that moves, 0 to 1.
 */
export function createConnection({
  onState,
  now = Date.now,
  random = Math.random,
  base = 1000,
  max = 30000,
  jitter = 0.25,
} = {}) {
  const flags = {
    signedOut: false,
    online: true,
    serviceDown: false,
    lost: false,
    slow: false,
    computer: 'here',
  };
  const listeners = new Set();
  let attempts = 0;
  let since = now();

  const derive = () => {
    if (flags.signedOut) return 'signed_out';
    if (!flags.online) return 'offline';
    if (flags.serviceDown) return 'service_down';
    if (flags.lost) return 'reconnecting';
    if (flags.computer === 'away') return 'asleep';
    if (flags.slow) return 'slow';
    return 'live';
  };

  let state = derive();

  const announce = (previous) => {
    const at = now();
    since = at;
    for (const listener of [...listeners]) listener(state, previous, at);
    onState?.(state, previous, at);
  };

  const set = () => {
    const next = derive();
    if (next === state) return state;
    const previous = state;
    state = next;
    announce(previous);
    return state;
  };

  /**
   * Report something that happened. Unknown events throw so a typo shows up in
   * development instead of silently saying nothing.
   */
  const report = (event, _detail) => {
    switch (event) {
      case 'answer':
      case 'ok':
        flags.online = true;
        flags.lost = false;
        flags.serviceDown = false;
        flags.slow = false;
        attempts = 0;
        break;
      case 'fast':
        flags.slow = false;
        break;
      case 'slow':
        flags.slow = true;
        break;
      case 'timeout':
        flags.lost = true;
        flags.slow = false;
        attempts += 1;
        break;
      case 'lost':
      case 'stream-closed':
        flags.lost = true;
        flags.slow = false;
        attempts += 1;
        break;
      case 'stream-open':
        flags.online = true;
        flags.lost = false;
        flags.serviceDown = false;
        flags.slow = false;
        attempts = 0;
        break;
      case 'reconnecting':
        flags.lost = true;
        break;
      case 'service-down':
      case 'server-error':
        flags.serviceDown = true;
        flags.lost = false;
        flags.slow = false;
        attempts += 1;
        break;
      case 'service-ok':
        flags.serviceDown = false;
        flags.lost = false;
        attempts = 0;
        break;
      case 'computer-away':
        flags.computer = 'away';
        break;
      case 'computer-here':
        flags.computer = 'here';
        break;
      case 'network-offline':
        flags.online = false;
        attempts += 1;
        break;
      case 'network-online':
        flags.online = true;
        flags.serviceDown = false;
        flags.slow = false;
        flags.lost = true;  // a stream does not survive a network drop
        attempts = 0;
        break;
      case 'signed-out':
        flags.signedOut = true;
        break;
      case 'signed-in':
        flags.signedOut = false;
        flags.lost = true;
        attempts = 0;
        break;
      default:
        throw new TypeError(`Unknown connection event: ${String(event)}`);
    }
    return set();
  };

  return {
    get state() {
      return state;
    },
    /** When the current state began. */
    get since() {
      return since;
    },
    /** Consecutive failures, for a caller that shows or logs them. */
    get attempts() {
      return attempts;
    },
    report,
    /** Wait before the next try, with jitter. The first failure waits `base`. */
    retryDelay() {
      return backoff(Math.max(0, attempts - 1), { base, max, jitter, random });
    },
    subscribe(fn, { immediate = false } = {}) {
      if (typeof fn !== 'function') throw new TypeError('A connection subscriber must be a function');
      listeners.add(fn);
      if (immediate) fn(state, null, since);
      return () => listeners.delete(fn);
    },
    /** Back to the state a fresh page starts in. */
    reset() {
      flags.signedOut = false;
      flags.online = true;
      flags.serviceDown = false;
      flags.lost = false;
      flags.slow = false;
      flags.computer = 'here';
      attempts = 0;
      set();
    },
  };
}

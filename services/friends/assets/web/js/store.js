// A very small reactive store.
//
// The page keeps one plain object of state. Views read a slice of it with a
// selector and subscribe to changes in that slice. Changes made in the same
// turn are announced once, in a microtask, and only to subscribers whose
// selected value actually changed. There is no diffing, no framework and no
// dependency on the DOM.

const defer = typeof queueMicrotask === 'function'
  ? queueMicrotask
  : fn => Promise.resolve().then(fn);

const asSelector = selector => {
  if (selector === undefined || selector === null) return state => state;
  if (typeof selector === 'function') return selector;
  if (typeof selector === 'string') return state => state[selector];
  throw new TypeError('A store selector must be a function, a key name or nothing');
};

/**
 * @param {object} [initial] The starting state. It is copied, not held.
 */
export function createStore(initial = {}) {
  let state = { ...initial };
  const subscribers = new Set();
  let queued = false;

  const announce = () => {
    queued = false;
    const current = state;
    const changes = [];
    for (const subscriber of [...subscribers]) {
      if (!subscribers.has(subscriber)) continue;  // unsubscribed during this turn
      let value;
      try {
        value = subscriber.selector(current);
      } catch (error) {
        changes.push({ subscriber, error });
        continue;
      }
      if (!subscriber.equal(value, subscriber.last)) {
        subscriber.last = value;
        changes.push({ subscriber, value });
      }
    }
    let firstError = null;
    for (const change of changes) {
      if (!subscribers.has(change.subscriber)) continue;
      if (change.error) {
        firstError ??= change.error;
        continue;
      }
      try {
        change.subscriber.fn(change.value, current);
      } catch (error) {
        firstError ??= error;
      }
    }
    if (firstError) throw firstError;  // surfaces as an uncaught microtask error
  };

  const schedule = () => {
    if (queued) return;
    queued = true;
    defer(announce);
  };

  return {
    /** The whole state, or the value a selector picks out of it. */
    get(selector) {
      return asSelector(selector)(state);
    },

    /**
     * Merge a patch into the state. A function receives the current state and
     * returns the patch. Returns the new state.
     */
    set(patch) {
      const next = typeof patch === 'function' ? patch(state) : patch;
      if (!next || typeof next !== 'object') return state;
      let changed = false;
      for (const [key, value] of Object.entries(next)) {
        if (!Object.is(state[key], value)) {
          changed = true;
          break;
        }
      }
      if (!changed) return state;
      state = { ...state, ...next };
      schedule();
      return state;
    },

    /**
     * Replace the state with what a function returns. Use `set` for a partial
     * change; use `update` when the next state depends on the whole of the old.
     */
    update(fn) {
      const next = fn(state);
      if (!next || typeof next !== 'object') return state;
      state = next;
      schedule();
      return state;
    },

    /**
     * Watch one selected value.
     *
     * @param {Function|string|null} selector
     * @param {(value: unknown, state: object) => void} fn
     * @param {{immediate?: boolean, equal?: (a: unknown, b: unknown) => boolean}} [options]
     * @returns {() => void} unsubscribe
     */
    subscribe(selector, fn, { immediate = false, equal = Object.is } = {}) {
      if (typeof fn !== 'function') throw new TypeError('A store subscriber must be a function');
      const subscriber = { selector: asSelector(selector), fn, equal, last: undefined };
      subscriber.last = subscriber.selector(state);
      subscribers.add(subscriber);
      if (immediate) fn(subscriber.last, state);
      return () => subscribers.delete(subscriber);
    },
  };
}

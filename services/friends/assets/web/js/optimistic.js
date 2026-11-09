// The optimistic interface rule, as a helper.
//
// A control shows its result the moment it is used. That shown value is held
// while the backend catches up. An answer that started before the user's latest
// action on that field is stale and ignored. If no answer arrives in time, the
// control asks again instead of flickering back to the old value.
//
// One helper holds one field per key (for example `shuffle`, `like` or `seek`).
// The caller starts an action with `press`, starts any other request with
// `begin`, passes the returned token to `answer`, and reads `shown`.

/**
 * @param {object} [options]
 * @param {number} [options.timeout] How long one shown value is held, in ms.
 * @param {() => number} [options.now] Clock, for tests.
 * @param {(fn: Function, ms: number) => unknown} [options.schedule] Timer, for tests.
 * @param {(handle: unknown) => void} [options.cancel] Timer canceller, for tests.
 * @param {(a: unknown, b: unknown) => boolean} [options.same] Value equality.
 * @param {(field: string, value: unknown, token: number) => void} [options.onExpire]
 *   Called once when a hold runs out. Ask again, then pass this token to
 *   `answer`; an answer from the request before the hold ran out is stale.
 */
export function createOptimistic({
  timeout = 4000,
  now = Date.now,
  schedule = (fn, ms) => setTimeout(fn, ms),
  cancel = handle => clearTimeout(handle),
  same = Object.is,
  onExpire = () => {},
} = {}) {
  const fields = new Map();

  const slot = field => {
    let found = fields.get(field);
    if (!found) {
      found = { seq: 0, token: 0, confirmed: undefined, held: null, timer: null };
      fields.set(field, found);
    }
    return found;
  };

  const clearHold = found => {
    if (found.timer !== null) {
      cancel(found.timer);
      found.timer = null;
    }
    found.held = null;
  };

  const expire = field => {
    const found = fields.get(field);
    if (!found || !found.held || found.held.expired) return;
    if (now() < found.held.until) return;  // an early timer changes nothing
    found.held.expired = true;
    found.timer = null;
    // Anything still in flight was asked before this question: it is stale.
    found.seq += 1;
    found.token = found.seq;
    onExpire(field, found.held.value, found.token);
  };

  return {
    /**
     * Start a request for a field without changing what it shows. Useful for
     * asking again, or for an action whose shown value is set elsewhere.
     * Returns the token that belongs to this request.
     */
    begin(field) {
      const found = slot(field);
      found.seq += 1;
      found.token = found.seq;
      return found.token;
    },

    /**
     * The user acted: show `value` now and hold it. Returns the token for the
     * answer to the request this action sends.
     */
    press(field, value, { timeout: holdFor = timeout } = {}) {
      const found = slot(field);
      found.seq += 1;
      found.token = found.seq;
      if (found.timer !== null) cancel(found.timer);
      found.held = { value, until: now() + holdFor, expired: false };
      if (holdFor > 0) {
        found.timer = schedule(() => expire(field), holdFor);
      }
      return found.token;
    },

    /**
     * Take in an answer. Returns false when the answer is stale (it started
     * before the latest action or re-ask on this field) and was ignored.
     *
     * Before the hold runs out, an answer that disagrees is remembered but the
     * shown value stays. An answer that agrees settles the field at once. Once
     * the hold has run out, the next answer is taken as the fresh truth.
     */
    answer(field, token, value) {
      const found = slot(field);
      if (token !== found.token) return false;
      if (found.held) {
        const expired = found.held.expired || now() >= found.held.until;
        if (same(found.held.value, value) || expired) clearHold(found);
      }
      found.confirmed = value;
      return true;
    },

    /**
     * What the control shows: the held value while there is one, else the
     * confirmed value. Pass the backend's latest value when there is one.
     */
    shown(field, confirmed) {
      const found = slot(field);
      if (confirmed !== undefined) {
        if (found.held && same(found.held.value, confirmed)) clearHold(found);
        else if (!found.held) found.confirmed = confirmed;
      }
      if (found.held && !found.held.expired && now() >= found.held.until) expire(field);
      if (found.held) return found.held.value;
      return confirmed !== undefined ? confirmed : found.confirmed;
    },

    /** Whether a shown value is still being held for an answer. */
    pending(field) {
      return !!fields.get(field)?.held;
    },

    /** Forget a field, for instance when its control leaves the page. */
    forget(field) {
      const found = fields.get(field);
      if (!found) return;
      clearHold(found);
      fields.delete(field);
    },

    /** Forget everything. */
    clear() {
      for (const found of fields.values()) clearHold(found);
      fields.clear();
    },
  };
}

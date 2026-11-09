// The queue's order, with no DOM and no audio.
//
// `list` is the play order and `index` points at the song playing (or about to
// play). `history` holds the songs that were current before it, most recent
// last, so Previous can go back where the listener actually came from, even
// after a jump or a shuffle. Everything here is plain data: copy it with
// `snapshot` and bring it back with `restore`.
//
// `next` defaults to a song that ended, which is when Repeat one keeps the
// same song. A hand pressed Next passes `{ ended: false }`, because changing
// songs by hand ends Repeat one (the desktop app has the same rule). Previous
// restarts the current song when there is nowhere earlier to go.
//
// Rows are not removed when they start: `list` is the order, `upcoming` is what
// still plays. Repeated songs are separate entries; keep them as separate
// values (the app uses song objects) when you need Previous to tell them apart.

export const REPEATS = Object.freeze(['off', 'all', 'one']);
const HISTORY_LIMIT = 200;

export function emptyQueue() {
  return { list: [], index: -1, history: [], repeat: 'off', shuffle: false, original: null };
}

const validIndex = (index, length) => {
  if (index === -1) return -1;  // a queue that finished with rows still in it
  return Number.isInteger(index) && index >= 0 && index < length ? index : (length ? 0 : -1);
};

/** A private, sane copy of any saved or partial state. */
function normalize(state) {
  const base = emptyQueue();
  if (!state || typeof state !== 'object') return base;
  base.list = Array.isArray(state.list) ? [...state.list] : [];
  base.index = validIndex(state.index, base.list.length);
  base.history = Array.isArray(state.history) ? [...state.history] : [];
  base.repeat = REPEATS.includes(state.repeat) ? state.repeat : 'off';
  base.shuffle = !!state.shuffle;
  base.original = Array.isArray(state.original) ? [...state.original] : null;
  if (!base.shuffle) base.original = null;
  return base;
}

const currentOf = state => (state.index >= 0 && state.index < state.list.length ? state.list[state.index] : undefined);
const upcomingOf = state => (state.index < 0 ? [] : state.list.slice(state.index + 1));

const rememberPlayed = state => {
  const entry = currentOf(state);
  if (entry === undefined) return;
  state.history.push(entry);
  if (state.history.length > HISTORY_LIMIT) state.history.splice(0, state.history.length - HISTORY_LIMIT);
};

const shuffled = (list, random) => {
  const copy = [...list];
  for (let at = copy.length - 1; at > 0; at--) {
    const take = Math.min(at, Math.max(0, Math.floor(random() * (at + 1))));
    [copy[at], copy[take]] = [copy[take], copy[at]];
  }
  return copy;
};

/** The song at the index, or undefined for an empty queue. */
export function current(state) {
  return currentOf(normalize(state));
}

/** The songs after the current one, in play order. */
export function upcoming(state) {
  return upcomingOf(normalize(state));
}

/**
 * Move on. With `ended` (the default, a song finishing) Repeat one keeps the
 * same song. At the end, Repeat all goes back to the top; otherwise the queue
 * reports `finished` and stays where it is. A hand skip passes `ended: false`.
 */
export function next(state, { ended = true } = {}) {
  const queue = normalize(state);
  if (!queue.list.length || queue.index < 0) {
    return { state: queue, moved: false, finished: true, entry: null };
  }
  const entry = currentOf(queue);
  if (ended && queue.repeat === 'one') {
    return { state: queue, moved: true, repeated: true, entry };
  }
  if (queue.index + 1 < queue.list.length) {
    rememberPlayed(queue);
    queue.index += 1;
    return { state: queue, moved: true, entry: currentOf(queue) };
  }
  if (queue.repeat === 'all') {
    rememberPlayed(queue);
    queue.index = 0;
    return { state: queue, moved: true, wrapped: true, entry: currentOf(queue) };
  }
  return { state: queue, moved: false, finished: true, entry: null };
}

/**
 * Go back to the song that was playing before this one. Where a jump or a
 * shuffle left a known history, that is used first. With nothing behind it,
 * the current song restarts.
 */
export function previous(state) {
  const queue = normalize(state);
  if (!queue.list.length || queue.index < 0) {
    return { state: queue, moved: false, restart: true, entry: null };
  }
  while (queue.history.length) {
    const wanted = queue.history.pop();
    const at = queue.list.findIndex(entry => Object.is(entry, wanted));
    if (at >= 0 && at !== queue.index) {
      queue.index = at;
      return { state: queue, moved: true, entry: currentOf(queue) };
    }
  }
  if (queue.index > 0) {
    queue.index -= 1;
    return { state: queue, moved: true, entry: currentOf(queue) };
  }
  return { state: queue, moved: false, restart: true, entry: currentOf(queue) };
}

/**
 * Play a row straight away, as tapping it in the queue does. `target` is an
 * index or the row itself. Rows above it are left behind but are not played.
 */
export function jump(state, target) {
  const queue = normalize(state);
  const at = typeof target === 'number'
    ? Math.trunc(target)
    : queue.list.findIndex(entry => Object.is(entry, target));
  if (!Number.isInteger(at) || at < 0 || at >= queue.list.length) {
    return { state: queue, moved: false, entry: currentOf(queue) };
  }
  if (at === queue.index) {
    return { state: queue, moved: false, restart: true, entry: queue.list[at] };
  }
  rememberPlayed(queue);
  queue.index = at;
  return { state: queue, moved: true, entry: queue.list[at] };
}

/** Queue songs next, in the order given, before anything already queued after. */
export function addNext(state, items) {
  const queue = normalize(state);
  const adding = Array.isArray(items) ? [...items] : [items];
  if (!adding.length) return { state: queue, added: 0 };
  if (queue.index < 0) {
    // Nothing is playing: the songs queued next are where playback starts.
    const start = queue.list.length;
    queue.list.push(...adding);
    queue.index = start;
  } else {
    queue.list.splice(queue.index + 1, 0, ...adding);
  }
  if (queue.original) queue.original.push(...adding);
  return { state: queue, added: adding.length };
}

/** Queue songs at the end, in the order given. */
export function addLast(state, items) {
  const queue = normalize(state);
  const adding = Array.isArray(items) ? [...items] : [items];
  if (!adding.length) return { state: queue, added: 0 };
  const start = queue.list.length;
  queue.list.push(...adding);
  if (queue.index < 0) queue.index = start;
  if (queue.original) queue.original.push(...adding);
  return { state: queue, added: adding.length };
}

/**
 * Remove one row by its index. Removing the playing row makes the next row
 * current; removing the last song leaves an empty, finished queue.
 */
export function remove(state, at) {
  const queue = normalize(state);
  const index = Number(at);
  if (!Number.isInteger(index) || index < 0 || index >= queue.list.length) {
    return { state: queue, removed: undefined, moved: false };
  }
  const wasCurrent = index === queue.index;
  const [removed] = queue.list.splice(index, 1);
  if (queue.original) {
    const found = queue.original.findIndex(entry => Object.is(entry, removed));
    if (found >= 0) queue.original.splice(found, 1);
  }

  if (!queue.list.length) queue.index = -1;
  else if (index < queue.index) queue.index -= 1;
  else if (wasCurrent) queue.index = index < queue.list.length ? index : -1;

  return { state: queue, removed, moved: true };
}

/** Move one row to another place in the play order. The current song stays current. */
export function move(state, from, to) {
  const queue = normalize(state);
  const start = Math.trunc(Number(from));
  const end = Math.trunc(Number(to));
  if (!Number.isInteger(start) || !Number.isInteger(end)) return { state: queue, moved: false };
  if (start < 0 || start >= queue.list.length || queue.list.length < 2) {
    return { state: queue, moved: false };
  }
  const target = Math.min(Math.max(0, end), queue.list.length - 1);
  if (target === start) return { state: queue, moved: false };

  const [entry] = queue.list.splice(start, 1);
  queue.list.splice(target, 0, entry);

  if (start === queue.index) queue.index = target;
  else if (start < queue.index && target >= queue.index) queue.index -= 1;
  else if (start > queue.index && target <= queue.index) queue.index += 1;

  return { state: queue, moved: true, index: queue.index };
}

/** Set Repeat to `off`, `all` or `one`. Anything else is a mistake. */
export function setRepeat(state, mode) {
  if (!REPEATS.includes(mode)) throw new TypeError(`Unknown repeat mode: ${String(mode)}`);
  const queue = normalize(state);
  queue.repeat = mode;
  return queue;
}

/**
 * Turn Shuffle on or off. On, the song that is playing stays first and the
 * rest are shuffled; the pre-shuffle order is kept. Off, that order comes back
 * with the song that is playing now as the current row.
 */
export function setShuffle(state, on, { random = Math.random } = {}) {
  const queue = normalize(state);
  const wanted = !!on;
  if (wanted === queue.shuffle) return queue;

  if (wanted) {
    queue.original = queue.original ?? [...queue.list];
    if (queue.list.length && queue.index >= 0) {
      const playing = queue.list[queue.index];
      const rest = queue.list.filter((_, at) => at !== queue.index);
      queue.list = [playing, ...shuffled(rest, random)];
      queue.index = 0;
    } else if (queue.list.length) {
      queue.list = shuffled(queue.list, random);
      // a queue that had finished stays finished
    }
    queue.shuffle = true;
  } else {
    if (queue.original) {
      const playing = currentOf(queue);
      queue.list = [...queue.original];
      const at = playing === undefined ? -1 : queue.list.findIndex(entry => Object.is(entry, playing));
      queue.index = at >= 0 ? at : validIndex(queue.index, queue.list.length);
    }
    queue.shuffle = false;
    queue.original = null;
  }
  return queue;
}

/** Restore a saved queue, ignoring anything that no longer makes sense. */
export function restore(saved) {
  return normalize(saved);
}

/** A copy that can be written out as JSON. */
export function snapshot(state) {
  return normalize(state);
}

/**
 * A queue that keeps its own current state. The pure functions above remain
 * available for callers that would rather pass state around themselves.
 *
 * @param {object|Array} [initial] A saved snapshot, or a list to start at row 0.
 */
export function createQueue(initial = {}) {
  let state = Array.isArray(initial)
    ? { ...emptyQueue(), list: [...initial], index: initial.length ? 0 : -1 }
    : normalize(initial);

  const run = fn => (...args) => {
    const out = fn(state, ...args);
    state = out.state;
    return out;
  };

  const queue = {
    get state() { return state; },
    get list() { return state.list; },
    get index() { return state.index; },
    get history() { return state.history; },
    get repeat() { return state.repeat; },
    get shuffle() { return state.shuffle; },
    get current() { return currentOf(state); },
    get upcoming() { return upcomingOf(state); },
    next: run(next),
    previous: run(previous),
    jump: run(jump),
    addNext: run(addNext),
    addLast: run(addLast),
    remove: run(remove),
    move: run(move),
    setRepeat(mode) {
      state = setRepeat(state, mode);
      return state.repeat;
    },
    setShuffle(on, options) {
      state = setShuffle(state, on, options);
      return state.shuffle;
    },
    toggleShuffle(options) {
      state = setShuffle(state, !state.shuffle, options);
      return state.shuffle;
    },
    snapshot() {
      return snapshot(state);
    },
    restore(saved) {
      state = normalize(saved);
      return queue;
    },
  };
  return queue;
}

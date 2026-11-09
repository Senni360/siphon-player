// Snapshots of what the service last said, kept in IndexedDB.
//
// A snapshot is read with its age so a screen can draw at once and ask again
// behind it (stale-while-revalidate). Writes are bounded: the oldest entries
// go first. If IndexedDB is unavailable or throws at any point — private
// browsing, a quota, a browser that does not offer it — the same calls keep
// working in memory for as long as the page lives.
//
// The cache stores plain values, which IndexedDB copies with structured
// cloning. A size is the length of the value's JSON in bytes, by default.

const DB_VERSION = 1;
const DEFAULT_STORE = 'snapshots';

const defaultSize = value => {
  try {
    const text = JSON.stringify(value);
    if (text === undefined) return Infinity;
    return typeof TextEncoder === 'function' ? new TextEncoder().encode(text).length : text.length;
  } catch {
    return Infinity;  // a value that cannot be cloned is not worth a cache slot
  }
};

/** Turn one IndexedDB request into a promise. */
function asked(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error('IndexedDB refused a request'));
  });
}

/**
 * @param {object} [options]
 * @param {string} [options.name] Database name.
 * @param {string} [options.storeName] Object store name.
 * @param {number} [options.maxBytes] Bound for all snapshots together.
 * @param {IDBFactory|null} [options.indexedDB] Defaults to the global one.
 * @param {() => number} [options.now] Clock, for tests.
 * @param {(value: unknown) => number} [options.sizeOf] Size of a value.
 */
export function createCache({
  name = 'encore-cache',
  storeName = DEFAULT_STORE,
  maxBytes = 2 * 1024 * 1024,
  indexedDB = globalThis.indexedDB,
  now = Date.now,
  sizeOf = defaultSize,
} = {}) {
  const memory = new Map();  // key -> { value, at, size }
  let database = null;
  let opening = null;
  let broken = !indexedDB;

  const giveUp = () => {
    broken = true;
    try {
      database?.close?.();
    } catch {
      // already gone
    }
    database = null;
  };

  async function opened() {
    if (broken || !indexedDB) return null;
    if (database) return database;
    if (opening) return opening;
    opening = (async () => {
      try {
        const request = indexedDB.open(name, DB_VERSION);
        request.onupgradeneeded = () => {
          try {
            if (!request.result.objectStoreNames.contains(storeName)) {
              request.result.createObjectStore(storeName, { keyPath: 'key' });
            }
          } catch {
            // a store that already exists, or a factory that will fail below
          }
        };
        database = await asked(request);
        return database;
      } catch {
        giveUp();
        return null;
      } finally {
        opening = null;
      }
    })();
    return opening;
  }

  const boundMemory = () => {
    let total = 0;
    for (const record of memory.values()) total += record.size;
    if (total <= maxBytes) return;
    const oldest = [...memory.entries()].sort((a, b) => a[1].at - b[1].at);
    for (const [key, record] of oldest) {
      if (total <= maxBytes) break;
      memory.delete(key);
      total -= record.size;
    }
  };

  const remember = (key, value, size, at) => {
    memory.delete(key);
    memory.set(key, { value, at, size });
    boundMemory();
  };

  return {
    /** Whether snapshots survive a reload, or only this page. */
    get persistent() {
      return !broken;
    },

    /**
     * The stored value and how long ago it was written, or null. Age is in ms.
     * The result is a plain `{ value, age }` either way.
     */
    async read(key) {
      if (!broken) {
        const db = await opened();
        if (db) {
          try {
            const record = await asked(
              db.transaction(storeName, 'readonly').objectStore(storeName).get(key),
            );
            if (record) {
              return { value: record.value, age: Math.max(0, now() - record.at) };
            }
          } catch {
            giveUp();
          }
        }
      }
      const held = memory.get(key);
      return held ? { value: held.value, age: Math.max(0, now() - held.at) } : null;
    },

    /**
     * Store a snapshot, evicting the oldest entries to stay under the bound.
     * Resolves to false only when the value alone is larger than the bound.
     */
    async write(key, value) {
      const size = sizeOf(value);
      if (!Number.isFinite(size) || size > maxBytes) return false;
      const at = now();

      if (!broken) {
        const db = await opened();
        if (db) {
          try {
            const all = await asked(
              db.transaction(storeName, 'readonly').objectStore(storeName).getAll(),
            );
            const others = all
              .filter(record => record.key !== key)
              .map(record => ({ ...record, size: record.size ?? sizeOf(record.value) }));
            let total = size + others.reduce((sum, record) => sum + record.size, 0);
            others.sort((a, b) => a.at - b.at);
            const evicted = [];
            while (total > maxBytes && others.length) {
              const oldest = others.shift();
              evicted.push(oldest.key);
              total -= oldest.size;
            }

            const write = db.transaction(storeName, 'readwrite').objectStore(storeName);
            const tasks = [asked(write.put({ key, value, at, size }))];
            for (const old of evicted) tasks.push(asked(write.delete(old)));
            await Promise.all(tasks);
            return true;
          } catch {
            giveUp();  // quota or a closed database: keep the value in memory
          }
        }
      }

      remember(key, value, size, at);
      return true;
    },

    /** Remove one snapshot. */
    async delete(key) {
      memory.delete(key);
      if (broken) return;
      const db = await opened();
      if (!db) return;
      try {
        await asked(db.transaction(storeName, 'readwrite').objectStore(storeName).delete(key));
      } catch {
        giveUp();
      }
    },

    /** Remove every snapshot, in both places. */
    async clear() {
      memory.clear();
      if (broken) return;
      const db = await opened();
      if (!db) return;
      try {
        await asked(db.transaction(storeName, 'readwrite').objectStore(storeName).clear());
      } catch {
        giveUp();
      }
    },
  };
}

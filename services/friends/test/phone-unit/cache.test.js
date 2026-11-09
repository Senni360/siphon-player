import assert from 'node:assert/strict';
import test from 'node:test';
import { createCache } from '../../assets/web/js/cache.js';

/** A little IndexedDB: enough for get, getAll, put, delete and clear. */
function request(work) {
  const made = {};
  queueMicrotask(() => {
    try {
      made.result = work();
      made.onsuccess?.();
    } catch (error) {
      made.error = error;
      made.onerror?.();
    }
  });
  return made;
}

function fakeIndexedDB({ failOpen = false } = {}) {
  const databases = new Map();
  const factory = {
    failWrites: false,
    open(name) {
      const made = {};
      queueMicrotask(() => {
        if (failOpen) {
          made.error = new Error('IndexedDB is not available here');
          made.onerror?.();
          return;
        }
        let database = databases.get(name);
        if (!database) {
          const stores = new Map();
          database = {
            objectStoreNames: { contains: store => stores.has(store) },
            createObjectStore(store) {
              stores.set(store, new Map());
              return {};
            },
            transaction(store, mode) {
              const data = stores.get(store);
              return {
                objectStore() {
                  return {
                    put(value) {
                      return request(() => {
                        if (mode === 'readwrite' && factory.failWrites) throw new Error('QuotaExceededError');
                        data.set(value.key, structuredClone(value));
                      });
                    },
                    get(key) {
                      return request(() => {
                        const found = data.get(key);
                        return found === undefined ? undefined : structuredClone(found);
                      });
                    },
                    getAll() {
                      return request(() => [...data.values()].map(value => structuredClone(value)));
                    },
                    delete(key) {
                      return request(() => {
                        if (mode === 'readwrite' && factory.failWrites) throw new Error('QuotaExceededError');
                        data.delete(key);
                      });
                    },
                    clear() {
                      return request(() => data.clear());
                    },
                  };
                },
              };
            },
            close() {},
          };
          databases.set(name, database);
          made.result = database;
          made.onupgradeneeded?.();
        } else {
          made.result = database;
        }
        made.onsuccess?.();
      });
      return made;
    },
  };
  return factory;
}

const clock = start => {
  let at = start;
  return {
    now: () => at,
    tick(ms) { at += ms; },
  };
};

test('a snapshot reads back with its age', async () => {
  const time = clock(1000);
  const cache = createCache({
    indexedDB: fakeIndexedDB(),
    now: time.now,
    sizeOf: value => value.length,
  });

  assert.equal(await cache.write('library', 'some songs'), true);
  time.tick(2500);

  assert.deepEqual(await cache.read('library'), { value: 'some songs', age: 2500 });
  assert.equal(await cache.read('nothing'), null);
  assert.equal(cache.persistent, true);
});

test('writing the same key replaces it and resets its age', async () => {
  const time = clock(0);
  const cache = createCache({ indexedDB: fakeIndexedDB(), now: time.now, sizeOf: value => value.length });

  await cache.write('state', 'one');
  time.tick(50);
  await cache.write('state', 'two');

  assert.deepEqual(await cache.read('state'), { value: 'two', age: 0 });
});

test('the oldest entries go first when the bound is crossed', async () => {
  const time = clock(0);
  const cache = createCache({
    indexedDB: fakeIndexedDB(),
    now: time.now,
    sizeOf: value => value.length,
    maxBytes: 10,
  });

  await cache.write('a', '11111');
  time.tick(1);
  await cache.write('b', '22222');
  time.tick(1);
  await cache.write('c', '33333');

  assert.equal(await cache.read('a'), null);
  assert.deepEqual(await cache.read('b'), { value: '22222', age: 1 });
  assert.deepEqual(await cache.read('c'), { value: '33333', age: 0 });
});

test('a value larger than the whole bound is not stored', async () => {
  const cache = createCache({ indexedDB: fakeIndexedDB(), sizeOf: value => value.length, maxBytes: 4 });

  assert.equal(await cache.write('big', '12345'), false);
  assert.equal(await cache.read('big'), null);
});

test('clear removes every snapshot', async () => {
  const cache = createCache({ indexedDB: fakeIndexedDB(), sizeOf: value => value.length });

  await cache.write('one', 'a');
  await cache.write('two', 'b');
  await cache.clear();

  assert.equal(await cache.read('one'), null);
  assert.equal(await cache.read('two'), null);
});

test('delete removes one snapshot', async () => {
  const cache = createCache({ indexedDB: fakeIndexedDB(), sizeOf: value => value.length });

  await cache.write('one', 'a');
  await cache.write('two', 'b');
  await cache.delete('one');

  assert.equal(await cache.read('one'), null);
  assert.equal((await cache.read('two'))?.value, 'b');
});

test('without IndexedDB everything still works in memory', async () => {
  const time = clock(0);
  const cache = createCache({ indexedDB: undefined, now: time.now, sizeOf: value => value.length });

  assert.equal(cache.persistent, false);
  await cache.write('library', 'songs');
  time.tick(10);
  assert.deepEqual(await cache.read('library'), { value: 'songs', age: 10 });

  await cache.clear();
  assert.equal(await cache.read('library'), null);
});

test('a database that will not open falls back to memory', async () => {
  const cache = createCache({
    indexedDB: fakeIndexedDB({ failOpen: true }),
    sizeOf: value => value.length,
  });

  assert.equal(await cache.write('state', 'here'), true);
  assert.equal(cache.persistent, false);
  const held = await cache.read('state');
  assert.equal(held.value, 'here');
  assert.ok(held.age >= 0);
});

test('a quota error falls back and keeps the newest value', async () => {
  const factory = fakeIndexedDB();
  const cache = createCache({ indexedDB: factory, sizeOf: value => value.length });

  await cache.write('first', 'hello');
  assert.equal(cache.persistent, true);

  factory.failWrites = true;
  assert.equal(await cache.write('second', 'world'), true);
  assert.equal(cache.persistent, false);
  assert.equal((await cache.read('second'))?.value, 'world');
});

test('the database opens once for many reads', async () => {
  let opens = 0;
  const factory = fakeIndexedDB();
  const original = factory.open;
  factory.open = (...args) => {
    opens += 1;
    return original(...args);
  };
  factory.failWrites = false;
  const cache = createCache({ indexedDB: factory, sizeOf: value => value.length });

  await cache.read('one');
  await cache.read('two');
  assert.equal(opens, 1);
});

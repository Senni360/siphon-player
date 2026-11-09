// A tiny in-memory TTL cache. It only ever holds metadata for the life of the
// process; nothing here is written to disk or logged.

export class TtlCache {
  #ttlMs;
  #maxEntries;
  #now;
  #entries = new Map();

  constructor({ ttlMs = 60_000, maxEntries = 500, now = Date.now } = {}) {
    this.#ttlMs = Math.max(0, ttlMs);
    this.#maxEntries = Math.max(0, maxEntries);
    this.#now = now;
  }

  get size() {
    return this.#entries.size;
  }

  has(key) {
    return this.get(key) !== undefined;
  }

  get(key) {
    const entry = this.#entries.get(key);
    if (!entry) return undefined;
    if (entry.expiresAt <= this.#now()) {
      this.#entries.delete(key);
      return undefined;
    }
    // Reading refreshes insertion order so the eviction is least-recently-used
    // within a process, which is close enough for metadata.
    this.#entries.delete(key);
    this.#entries.set(key, entry);
    return entry.value;
  }

  set(key, value, ttlMs = this.#ttlMs) {
    if (ttlMs <= 0) return value;
    if (this.#maxEntries === 0) return value;
    if (this.#entries.has(key)) this.#entries.delete(key);
    while (this.#entries.size >= this.#maxEntries) {
      const oldest = this.#entries.keys().next().value;
      if (oldest === undefined) break;
      this.#entries.delete(oldest);
    }
    this.#entries.set(key, { value, expiresAt: this.#now() + ttlMs });
    return value;
  }

  delete(key) {
    return this.#entries.delete(key);
  }

  clear() {
    this.#entries.clear();
  }
}

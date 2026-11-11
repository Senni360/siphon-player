// Sources that cannot be played. SoundCloud serves some tracks only as DRM
// protected audio, and yt-dlp refuses those, correctly. Remembering the source
// keeps every later play from spending a resolve on a copy that can never work.
//
// The set is small, bounded and persistent. Entries are forgotten after thirty
// days, so a track that becomes available is found again. Reason and time are
// the only things stored: never a title, an uploader or a listener.

import path from "node:path";
import { atomicWriteJson, readJsonFile } from "./atomic.mjs";

export const MAX_ENTRIES = 5000;
export const TTL_MS = 30 * 24 * 60 * 60 * 1000;
const REASON = /^[a-z][a-z0-9_]{0,39}$/;

export class UnplayableStore {
  #file;
  #now;
  #maxEntries;
  #ttlMs;
  #logger;
  #entries = new Map();
  #writing = Promise.resolve();

  constructor({ dir, now = Date.now, maxEntries = MAX_ENTRIES, ttlMs = TTL_MS, logger } = {}) {
    this.#file = path.join(dir, "unplayable.json");
    this.#now = now;
    this.#maxEntries = Math.max(1, maxEntries);
    this.#ttlMs = Math.max(1, ttlMs);
    this.#logger = logger;
  }

  /**
   * Loads the stored set. A missing file is empty, and so is a corrupt or
   * unreadable one: the set only saves time, so it never blocks start-up.
   */
  async init() {
    let stored = null;
    try {
      stored = await readJsonFile(this.#file, null);
    } catch {
      stored = null;
    }
    const sources = stored?.sources;
    if (sources && typeof sources === "object" && !Array.isArray(sources)) {
      for (const [sourceUri, entry] of Object.entries(sources)) {
        if (typeof sourceUri !== "string" || sourceUri.length === 0 || sourceUri.length > 512) continue;
        const at = Number(entry?.at);
        if (!Number.isFinite(at)) continue;
        const reason = typeof entry?.reason === "string" && REASON.test(entry.reason) ? entry.reason : "unplayable";
        this.#entries.set(sourceUri, { reason, at });
        while (this.#entries.size > this.#maxEntries) {
          const oldest = this.#entries.keys().next().value;
          if (oldest === undefined) break;
          this.#entries.delete(oldest);
        }
      }
    }
    this.#prune();
    return this;
  }

  /** The stored reason for a source, or null when it is missing or expired. */
  get(sourceUri) {
    const entry = typeof sourceUri === "string" ? this.#entries.get(sourceUri) : undefined;
    if (!entry) return null;
    if (this.#now() - entry.at >= this.#ttlMs) {
      this.#entries.delete(sourceUri);
      return null;
    }
    return { reason: entry.reason, at: entry.at };
  }

  has(sourceUri) {
    return this.get(sourceUri) !== null;
  }

  /** Remembers a source, dropping the oldest entries past the bound. */
  async add(sourceUri, reason = "unplayable") {
    if (typeof sourceUri !== "string" || sourceUri.length === 0 || sourceUri.length > 512) return false;
    this.#prune();
    this.#entries.delete(sourceUri);
    this.#entries.set(sourceUri, {
      reason: typeof reason === "string" && REASON.test(reason) ? reason : "unplayable",
      at: this.#now(),
    });
    while (this.#entries.size > this.#maxEntries) {
      const oldest = this.#entries.keys().next().value;
      if (oldest === undefined) break;
      this.#entries.delete(oldest);
    }
    await this.flush();
    return true;
  }

  #prune(now = this.#now()) {
    for (const [sourceUri, entry] of this.#entries) {
      if (now - entry.at >= this.#ttlMs) this.#entries.delete(sourceUri);
    }
  }

  /** Writes the set in insertion order, so the oldest entry stays the oldest. */
  flush() {
    const run = async () => {
      const sources = {};
      for (const [sourceUri, entry] of this.#entries) sources[sourceUri] = entry;
      await atomicWriteJson(this.#file, { version: 1, sources });
    };
    const next = this.#writing.then(run, run);
    this.#writing = next.catch((error) => {
      this.#logger?.warn("unplayable_write_failed", { code: error?.code ?? "error" });
    });
    return this.#writing;
  }

  stats() {
    return { sources: this.#entries.size };
  }
}

// A disposable rolling cache. Files are named by a key that embeds a hash of
// the owner, which lets media serving prove ownership without an index.
// Least-recently-played entries are evicted first; uploads live elsewhere.
//
// A file only ever appears under its final name after a successful fsync, so a
// crash cannot leave a truncated track that later looks valid. A file that is
// still being fetched lives beside it as `<key>.<ext>.part` and can be read
// through `begin`/`progress`/`complete`; streams pin it so eviction cannot pull
// the bytes out from under a phone. Temp files and `.part` copies are removed
// on start.

import fs from "node:fs/promises";
import path from "node:path";
import {
  atomicWriteJson,
  cacheKey,
  CorruptJsonError,
  ensureDir,
  fsyncFile,
  readJsonFile,
  removeMatching,
  syncDir,
} from "./atomic.mjs";

export const CACHE_KEY = /^c[0-9a-f]{56}$/;
const EXTENSIONS = new Map([
  ["m4a", { ext: ".m4a", contentType: "audio/mp4" }],
  ["mp3", { ext: ".mp3", contentType: "audio/mpeg" }],
  ["ogg", { ext: ".ogg", contentType: "audio/ogg" }],
  ["flac", { ext: ".flac", contentType: "audio/flac" }],
  ["wav", { ext: ".wav", contentType: "audio/wav" }],
]);
const CACHE_FILE = /^(c[0-9a-f]{56})\.(m4a|mp3|ogg|flac|wav)$/;
const MIB = 1024 ** 2;

export class CacheError extends Error {
  constructor(code, status = 500) {
    super(code);
    this.name = "CacheError";
    this.code = code;
    this.status = status;
  }
}

function extensionFor(contentType) {
  if (contentType === "audio/mpeg") return EXTENSIONS.get("mp3");
  if (contentType === "audio/ogg") return EXTENSIONS.get("ogg");
  if (contentType === "audio/flac") return EXTENSIONS.get("flac");
  if (contentType === "audio/wav") return EXTENSIONS.get("wav");
  return EXTENSIONS.get("m4a");
}

function diskIsFull(error) {
  return error?.code === "ENOSPC" || error?.code === "EDQUOT";
}

export class CacheStore {
  #dir;
  #maxBytes;
  #now;
  #entries = new Map();
  #pending = new Map();
  #pins = new Map();
  #deferredDelete = new Set();
  #evicting = null;
  #total = 0;
  #dirty = false;
  #flushTimer = null;

  constructor({ dir, maxBytes = 4 * 1024 ** 3, now = Date.now }) {
    this.#dir = path.join(dir, "cache");
    this.#maxBytes = maxBytes;
    this.#now = now;
  }

  get dir() {
    return this.#dir;
  }

  get maxBytes() {
    return this.#maxBytes;
  }

  keyFor(sub, sourceUri) {
    return cacheKey(sub, sourceUri);
  }

  ownerOf(key) {
    return CACHE_KEY.test(key) ? key.slice(1, 17) : "";
  }

  async init() {
    await ensureDir(this.#dir);
    // A `.part`/temp file can only be a write that never finished.
    await removeMatching(this.#dir, /\.part$/);
    await removeMatching(this.#dir, /^\.tmp-/);
    let lastPlayed = {};
    try {
      lastPlayed = await readJsonFile(path.join(this.#dir, "last-played.json"), {});
    } catch (error) {
      // Only the play order is lost, and it is rebuilt from the directory.
      if (!(error instanceof CorruptJsonError)) throw error;
      lastPlayed = {};
    }
    const names = await fs.readdir(this.#dir).catch((error) => {
      if (error?.code === "ENOENT") return [];
      throw error;
    });
    for (const name of names) {
      const match = CACHE_FILE.exec(name);
      if (!match) continue;
      const file = path.join(this.#dir, name);
      let stat;
      try {
        stat = await fs.stat(file);
      } catch {
        continue;
      }
      if (!stat.isFile() || stat.size === 0) {
        await fs.unlink(file).catch(() => {});
        continue;
      }
      const contentType = EXTENSIONS.get(match[2]).contentType;
      const played = Number(lastPlayed?.[match[1]]);
      this.#entries.set(match[1], {
        key: match[1],
        file,
        size: stat.size,
        mtimeMs: stat.mtimeMs,
        contentType,
        lastPlayed: Number.isFinite(played) ? played : stat.atimeMs || stat.mtimeMs,
      });
      this.#total += stat.size;
    }
    await this.enforce();
    return this;
  }

  get(key) {
    return this.#entries.get(key) ?? null;
  }

  has(key) {
    return this.#entries.has(key) || this.#pending.has(key);
  }

  /** The state a media reader needs, whether the file is finished or growing. */
  status(key) {
    const pending = this.#pending.get(key);
    if (pending) {
      return {
        key,
        file: pending.file,
        contentType: pending.contentType,
        size: pending.size,
        mtimeMs: pending.createdAt,
        complete: false,
        error: pending.error,
      };
    }
    const entry = this.#entries.get(key);
    if (!entry) return null;
    return {
      key,
      file: entry.file,
      contentType: entry.contentType,
      size: entry.size,
      mtimeMs: entry.mtimeMs,
      complete: true,
      error: null,
    };
  }

  pending(key) {
    return this.#pending.get(key) ?? null;
  }

  touch(key) {
    const entry = this.#entries.get(key);
    if (!entry) return false;
    entry.lastPlayed = this.#now();
    this.#markDirty();
    return true;
  }

  /**
   * Registers a file that is being fetched. The caller creates `entry.file`
   * with `wx` and writes through `progress`; nothing is visible under the
   * final name until `complete` renames it.
   */
  async begin(key, { contentType = "audio/mp4" } = {}) {
    if (!CACHE_KEY.test(key)) throw new Error("invalid_cache_key");
    const existing = this.#pending.get(key);
    if (existing) return existing;
    const finished = this.#entries.get(key);
    if (finished) return finished;
    const format = extensionFor(contentType);
    await ensureDir(this.#dir);
    const entry = {
      key,
      file: path.join(this.#dir, `${key}${format.ext}.part`),
      contentType: format.contentType,
      size: 0,
      complete: false,
      error: null,
      createdAt: this.#now(),
      waiters: new Set(),
    };
    this.#pending.set(key, entry);
    return entry;
  }

  /** Records bytes written so far and wakes any reader waiting for them. */
  progress(key, size) {
    const entry = this.#pending.get(key);
    if (!entry || !Number.isSafeInteger(size) || size < 0) return false;
    if (size > entry.size) entry.size = size;
    this.#notify(entry);
    return true;
  }

  /**
   * Resolves once `minBytes` are written (or the file is complete), rejecting
   * if the fetch failed or the signal aborted. Returns the same shape as
   * `status`. A reader that is already at the end of a growing file simply
   * waits for the next chunk.
   */
  waitForBytes(key, minBytes, signal) {
    const current = this.status(key);
    if (!current) return Promise.reject(new CacheError("not_found", 404));
    if (current.error) return Promise.reject(current.error);
    if (current.complete || current.size >= minBytes) return Promise.resolve(current);
    const pending = this.#pending.get(key);
    if (!pending) return Promise.resolve(current);
    return new Promise((resolve, reject) => {
      let finished = false;
      const stop = (error, value) => {
        if (finished) return;
        finished = true;
        pending.waiters.delete(waiter);
        signal?.removeEventListener("abort", onAbort);
        if (error) reject(error);
        else resolve(value);
      };
      const onAbort = () => {
        const error = new Error("aborted");
        error.name = "AbortError";
        stop(error);
      };
      const waiter = () => {
        if (finished) return true;
        const state = this.status(key);
        if (!state) {
          stop(new CacheError("not_found", 404));
          return true;
        }
        if (state.error) {
          stop(state.error);
          return true;
        }
        if (state.complete || state.size >= minBytes) {
          stop(null, state);
          return true;
        }
        return false;
      };
      pending.waiters.add(waiter);
      if (signal) {
        if (signal.aborted) {
          onAbort();
          return;
        }
        signal.addEventListener("abort", onAbort, { once: true });
      }
    });
  }

  /**
   * Finishes a fetch: fsyncs the bytes, renames the `.part` file to its final
   * name, flushes the directory and indexes it. A crash before the rename
   * leaves only a `.part` file, which `init` sweeps.
   */
  async complete(key) {
    const pending = this.#pending.get(key);
    if (!pending) {
      const existing = this.#entries.get(key);
      if (existing) return existing;
      throw new CacheError("not_found", 404);
    }
    let handle;
    try {
      handle = await fs.open(pending.file, "r+");
      await handle.sync().catch((error) => {
        if (!["EPERM", "EACCES", "EINVAL", "ENOTSUP", "ENOSYS"].includes(error?.code)) throw error;
      });
      await handle.close();
      handle = undefined;
      const stat = await fs.stat(pending.file);
      if (!stat.isFile() || stat.size === 0) throw new CacheError("empty_cache_file", 502);
      const target = pending.file.slice(0, -".part".length);
      await this.#moveIntoPlace(pending.file, target);
      await syncDir(this.#dir);
      this.#pending.delete(key);
      const entry = {
        key,
        file: target,
        size: stat.size,
        mtimeMs: stat.mtimeMs,
        contentType: pending.contentType,
        lastPlayed: this.#now(),
      };
      this.#entries.set(key, entry);
      this.#total += stat.size;
      this.#notify(pending, { complete: true });
      this.#markDirty();
      await this.enforce();
      return entry;
    } catch (error) {
      if (handle) await handle.close().catch(() => {});
      await this.#discardPending(pending, error);
      throw error;
    }
  }

  /**
   * Renames over the target. Windows refuses a rename while a reader has the
   * file open for a moment, so a short retry loop is part of the write path,
   * not an error.
   */
  async #moveIntoPlace(from, to) {
    for (let attempt = 0; attempt < 20; attempt += 1) {
      try {
        await fs.rename(from, to);
        return;
      } catch (error) {
        if (error?.code === "EEXIST" || error?.code === "ENOTEMPTY") {
          await fs.unlink(to).catch(() => {});
          continue;
        }
        if ((error?.code === "EPERM" || error?.code === "EACCES" || error?.code === "EBUSY") && attempt < 19) {
          await new Promise((resolve) => setTimeout(resolve, 10));
          continue;
        }
        throw error;
      }
    }
    throw new CacheError("cache_rename_failed", 500);
  }

  /** Abandons a fetch and removes its partial file. */
  async abort(key, error = new CacheError("aborted", 499)) {
    const pending = this.#pending.get(key);
    if (!pending) return false;
    await this.#discardPending(pending, error);
    return true;
  }

  async #discardPending(pending, error) {
    if (this.#pending.get(pending.key) === pending) this.#pending.delete(pending.key);
    pending.error = error instanceof Error ? error : new Error(String(error));
    this.#notify(pending);
    await fs.unlink(pending.file).catch(() => {});
  }

  #notify(pending, extra = {}) {
    if (pending.error) {
      for (const waiter of [...pending.waiters]) {
        try {
          waiter();
        } catch {
          // A reader that already gave up.
        }
      }
      return;
    }
    if (extra.complete) pending.complete = !pending.error;
    for (const waiter of [...pending.waiters]) {
      try {
        if (waiter()) pending.waiters.delete(waiter);
      } catch {
        pending.waiters.delete(waiter);
      }
    }
  }

  /**
   * Moves a finished file into the cache under `key`. The caller has already
   * fsynced the bytes; the temp directory is on the same volume, so the move
   * is one atomic rename. Only a filesystem error that is not a full disk is
   * passed through.
   */
  async put(key, sourceFile, { contentType = "audio/mp4" } = {}) {
    if (!CACHE_KEY.test(key)) throw new Error("invalid_cache_key");
    const format = extensionFor(contentType);
    await ensureDir(this.#dir);
    const target = path.join(this.#dir, `${key}${format.ext}`);
    const stat = await fs.stat(sourceFile);
    if (!stat.isFile() || stat.size === 0) throw new Error("empty_cache_file");
    try {
      await this.#place(sourceFile, target);
    } catch (error) {
      if (!diskIsFull(error)) throw error;
      // The volume, not the cache cap, is what ran out: make real room first,
      // then try the same move once more.
      await this.#evictForSpace();
      try {
        await this.#place(sourceFile, target);
      } catch (retry) {
        if (diskIsFull(retry)) throw new CacheError("cache_full", 507);
        throw retry;
      }
    }
    const previous = this.#entries.get(key);
    if (previous) {
      this.#total -= previous.size;
      this.#entries.delete(key);
      if (previous.file !== target) await fs.unlink(previous.file).catch(() => {});
    }
    const moved = await fs.stat(target);
    this.#entries.set(key, {
      key,
      file: target,
      size: moved.size,
      mtimeMs: moved.mtimeMs,
      contentType: format.contentType,
      lastPlayed: this.#now(),
    });
    this.#total += moved.size;
    this.#markDirty();
    await this.enforce();
    return this.get(key);
  }

  async #place(sourceFile, target) {
    // The bytes must be durable before the name can point at them.
    await fsyncFile(sourceFile);
    try {
      await fs.rename(sourceFile, target);
      await syncDir(this.#dir);
      return;
    } catch (error) {
      if (error?.code !== "EXDEV") throw error;
    }
    // Another filesystem: copy beside the target first, so a failed copy never
    // leaves a short file under the real name.
    const part = `${target}.part`;
    let handle;
    try {
      await fs.copyFile(sourceFile, part);
      handle = await fs.open(part, "r+");
      await handle.sync();
      await handle.close();
      handle = undefined;
      await this.#moveIntoPlace(part, target);
      await syncDir(this.#dir);
    } catch (error) {
      await handle?.close().catch(() => {});
      await fs.unlink(part).catch(() => {});
      throw error;
    }
    await fs.unlink(sourceFile).catch(() => {});
  }

  /** Best effort before answering 507: give back at least 512 MiB or 10%. */
  async #evictForSpace() {
    const wanted = Math.max(512 * MIB, this.#maxBytes * 0.1);
    const target = Math.max(0, this.#total - wanted);
    return this.#evictTo(target);
  }

  async enforce() {
    if (this.#total <= this.#maxBytes) return { evicted: 0, bytes: 0 };
    return this.#evictTo(this.#maxBytes);
  }

  /**
   * Runs evictions one at a time. Two puts that race `enforce` must not both
   * account for the same entry, or the in-memory total drifts below the disk.
   */
  #evictTo(limit) {
    const run = async () => {
      if (this.#total <= limit) return { evicted: 0, bytes: 0 };
      const ordered = [...this.#entries.values()].sort((a, b) => a.lastPlayed - b.lastPlayed);
      let evicted = 0;
      let bytes = 0;
      for (const snapshot of ordered) {
        if (this.#total <= limit) break;
        const entry = this.#entries.get(snapshot.key);
        if (!entry || entry !== snapshot) continue;
        // A file a phone is reading stays until the stream lets go.
        if (this.#pinned(entry.key)) continue;
        try {
          await fs.unlink(entry.file);
        } catch (error) {
          if (error?.code !== "ENOENT") throw error;
        }
        const removed = this.#dropEntry(entry);
        evicted += 1;
        bytes += removed;
      }
      if (evicted > 0) this.#markDirty();
      return { evicted, bytes };
    };
    const previous = this.#evicting ?? Promise.resolve();
    const next = previous.then(run, run);
    this.#evicting = next.catch(() => {});
    return next;
  }

  #dropEntry(entry) {
    const current = this.#entries.get(entry.key);
    if (current !== entry) return 0;
    this.#entries.delete(entry.key);
    this.#total -= current.size;
    return current.size;
  }

  /** Marks a key as in use by a reader; eviction skips it until unpinned. */
  pin(key) {
    if (!this.#entries.has(key) && !this.#pending.has(key)) return false;
    this.#pins.set(key, (this.#pins.get(key) ?? 0) + 1);
    return true;
  }

  unpin(key) {
    const count = this.#pins.get(key) ?? 0;
    if (count <= 0) return false;
    if (count > 1) {
      this.#pins.set(key, count - 1);
      return true;
    }
    this.#pins.delete(key);
    if (this.#deferredDelete.delete(key)) {
      const entry = this.#entries.get(key);
      if (entry) {
        fs.unlink(entry.file).catch(() => {}).finally(() => {
          this.#dropEntry(entry);
          this.#markDirty();
        });
      }
    } else {
      this.enforce().catch(() => {});
    }
    return true;
  }

  #pinned(key) {
    return (this.#pins.get(key) ?? 0) > 0;
  }

  async remove(key) {
    const pending = this.#pending.get(key);
    if (pending) {
      await this.abort(key);
      return true;
    }
    const entry = this.#entries.get(key);
    if (!entry) return false;
    if (this.#pinned(key)) {
      // The reader keeps the open file; delete it as soon as it finishes.
      this.#deferredDelete.add(key);
      return true;
    }
    await fs.unlink(entry.file).catch(() => {});
    this.#dropEntry(entry);
    this.#markDirty();
    return true;
  }

  #markDirty() {
    this.#dirty = true;
    if (this.#flushTimer) return;
    this.#flushTimer = setTimeout(() => {
      this.#flushTimer = null;
      this.flush().catch(() => {});
    }, 5_000);
    this.#flushTimer.unref?.();
  }

  async flush() {
    if (!this.#dirty) return;
    this.#dirty = false;
    const lastPlayed = {};
    for (const entry of this.#entries.values()) lastPlayed[entry.key] = entry.lastPlayed;
    await atomicWriteJson(path.join(this.#dir, "last-played.json"), lastPlayed);
  }

  async close() {
    if (this.#flushTimer) {
      clearTimeout(this.#flushTimer);
      this.#flushTimer = null;
    }
    await this.flush();
  }

  stats() {
    let pinned = 0;
    for (const count of this.#pins.values()) if (count > 0) pinned += 1;
    return {
      bytes: this.#total,
      capBytes: this.#maxBytes,
      files: this.#entries.size,
      pending: this.#pending.size,
      pinned,
    };
  }
}

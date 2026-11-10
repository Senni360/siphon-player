// A disposable rolling cache. Files are named by a key that embeds a hash of
// the owner, which lets media serving prove ownership without an index.
// Least-recently-played entries are evicted first; uploads live elsewhere.
//
// A file only ever appears under its final name after a successful fsync, so a
// crash cannot leave a truncated track that later looks valid. Temp files and
// `.part` copies are removed on start.

import fs from "node:fs/promises";
import path from "node:path";
import {
  atomicWriteJson,
  cacheKey,
  CorruptJsonError,
  ensureDir,
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
      let match = CACHE_FILE.exec(name);
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
    return this.#entries.has(key);
  }

  touch(key) {
    const entry = this.#entries.get(key);
    if (!entry) return false;
    entry.lastPlayed = this.#now();
    this.#markDirty();
    return true;
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
      await fs.rename(part, target);
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

  async #evictTo(limit) {
    if (this.#total <= limit) return { evicted: 0, bytes: 0 };
    const ordered = [...this.#entries.values()].sort((a, b) => a.lastPlayed - b.lastPlayed);
    let evicted = 0;
    let bytes = 0;
    for (const entry of ordered) {
      if (this.#total <= limit) break;
      try {
        await fs.unlink(entry.file);
      } catch (error) {
        if (error?.code !== "ENOENT") throw error;
      }
      this.#entries.delete(entry.key);
      this.#total -= entry.size;
      evicted += 1;
      bytes += entry.size;
    }
    if (evicted > 0) this.#markDirty();
    return { evicted, bytes };
  }

  async remove(key) {
    const entry = this.#entries.get(key);
    if (!entry) return false;
    await fs.unlink(entry.file).catch(() => {});
    this.#entries.delete(key);
    this.#total -= entry.size;
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
    return { bytes: this.#total, capBytes: this.#maxBytes, files: this.#entries.size };
  }
}

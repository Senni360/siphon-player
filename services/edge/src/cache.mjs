// A disposable rolling cache. Files are named by a key that embeds a hash of
// the owner, which lets media serving prove ownership without an index.
// Least-recently-played entries are evicted first; uploads live elsewhere.

import fs from "node:fs/promises";
import path from "node:path";
import { cacheKey, ensureDir, readJsonFile, atomicWriteJson } from "./atomic.mjs";

export const CACHE_KEY = /^c[0-9a-f]{56}$/;
const EXTENSIONS = new Map([
  ["m4a", { ext: ".m4a", contentType: "audio/mp4" }],
  ["mp3", { ext: ".mp3", contentType: "audio/mpeg" }],
  ["ogg", { ext: ".ogg", contentType: "audio/ogg" }],
  ["flac", { ext: ".flac", contentType: "audio/flac" }],
  ["wav", { ext: ".wav", contentType: "audio/wav" }],
]);

function extensionFor(contentType) {
  if (contentType === "audio/mpeg") return EXTENSIONS.get("mp3");
  if (contentType === "audio/ogg") return EXTENSIONS.get("ogg");
  if (contentType === "audio/flac") return EXTENSIONS.get("flac");
  if (contentType === "audio/wav") return EXTENSIONS.get("wav");
  return EXTENSIONS.get("m4a");
}

export class CacheStore {
  #dir;
  #maxBytes;
  #now;
  #entries = new Map();
  #total = 0;
  #dirty = false;
  #flushTimer = null;

  constructor({ dir, maxBytes = 10 * 1024 ** 3, now = Date.now }) {
    this.#dir = path.join(dir, "cache");
    this.#maxBytes = maxBytes;
    this.#now = now;
  }

  get dir() {
    return this.#dir;
  }

  keyFor(sub, sourceUri) {
    return cacheKey(sub, sourceUri);
  }

  ownerOf(key) {
    return CACHE_KEY.test(key) ? key.slice(1, 17) : "";
  }

  async init() {
    await ensureDir(this.#dir);
    const lastPlayed = await readJsonFile(path.join(this.#dir, "last-played.json"), {});
    const names = await fs.readdir(this.#dir).catch((error) => {
      if (error?.code === "ENOENT") return [];
      throw error;
    });
    for (const name of names) {
      const match = /^(c[0-9a-f]{56})\.(m4a|mp3|ogg|flac|wav)$/.exec(name);
      if (!match) continue;
      const file = path.join(this.#dir, name);
      let stat;
      try {
        stat = await fs.stat(file);
      } catch {
        continue;
      }
      if (!stat.isFile()) continue;
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

  async put(key, sourceFile, { contentType = "audio/mp4" } = {}) {
    if (!CACHE_KEY.test(key)) throw new Error("invalid_cache_key");
    const format = extensionFor(contentType);
    await ensureDir(this.#dir);
    const target = path.join(this.#dir, `${key}${format.ext}`);
    const stat = await fs.stat(sourceFile);
    if (!stat.isFile() || stat.size === 0) throw new Error("empty_cache_file");
    const previous = this.#entries.get(key);
    if (previous) {
      this.#total -= previous.size;
      this.#entries.delete(key);
      if (previous.file !== target) await fs.unlink(previous.file).catch(() => {});
    }
    try {
      await fs.rename(sourceFile, target);
    } catch (error) {
      if (error?.code !== "EXDEV") throw error;
      await fs.copyFile(sourceFile, target);
      await fs.unlink(sourceFile).catch(() => {});
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

  async enforce() {
    if (this.#total <= this.#maxBytes) return { evicted: 0, bytes: 0 };
    const ordered = [...this.#entries.values()].sort((a, b) => a.lastPlayed - b.lastPlayed);
    let evicted = 0;
    let bytes = 0;
    for (const entry of ordered) {
      if (this.#total <= this.#maxBytes) break;
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

// Small filesystem helpers shared by the sync store, cache and uploads.

import { createHash, randomBytes } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

export function userHash(sub) {
  return createHash("sha256").update(`encore-edge-user\0${sub}`).digest("hex");
}

export function cacheKey(sub, sourceUri) {
  const owner = userHash(sub).slice(0, 16);
  const digest = createHash("sha256").update(`encore-edge-cache\0${sourceUri}`).digest("hex").slice(0, 40);
  return `c${owner}${digest}`;
}

export function ownerPrefix(sub) {
  return userHash(sub).slice(0, 16);
}

/** Runs async work one at a time per key, preserving call order. */
export class KeyedQueue {
  #tails = new Map();
  run(key, operation) {
    const previous = this.#tails.get(key) ?? Promise.resolve();
    const next = previous.then(operation, operation);
    const tail = next.catch(() => {});
    this.#tails.set(key, tail);
    tail.finally(() => {
      if (this.#tails.get(key) === tail) this.#tails.delete(key);
    });
    return next;
  }
}

export async function ensureDir(dir) {
  await fs.mkdir(dir, { recursive: true, mode: 0o750 });
}

/**
 * A document that exists but is not valid JSON. It is never treated as
 * "absent": that would silently reset somebody's state to revision 0.
 */
export class CorruptJsonError extends Error {
  constructor(file) {
    super("corrupt_json");
    this.name = "CorruptJsonError";
    this.code = "corrupt_json";
    this.file = file;
  }
}

/**
 * Flushes a directory entry so a rename survives a power cut. Some platforms
 * refuse to open a directory; there it is skipped rather than failing a write.
 */
export async function syncDir(dir) {
  let handle;
  try {
    handle = await fs.open(dir, "r");
    await handle.sync();
  } catch {
    // Not supported here, or already durable enough.
  } finally {
    await handle?.close().catch(() => {});
  }
}

/**
 * Writes bytes to a fresh temp file in the same directory, fsyncs, renames
 * over the target and flushes the directory. A crash leaves the previous file
 * intact, and nothing half-written ever carries the final name.
 */
export async function atomicWriteFile(target, bytes, { mode = 0o600 } = {}) {
  const dir = path.dirname(target);
  await ensureDir(dir);
  const temp = path.join(dir, `.tmp-${process.pid}-${randomBytes(8).toString("hex")}`);
  let handle;
  try {
    handle = await fs.open(temp, "wx", mode);
    await handle.writeFile(bytes);
    await handle.sync();
    await handle.close();
    handle = undefined;
    await fs.rename(temp, target);
    await syncDir(dir);
  } catch (error) {
    if (handle) await handle.close().catch(() => {});
    await fs.unlink(temp).catch(() => {});
    throw error;
  }
}

export async function atomicWriteJson(target, value) {
  await atomicWriteFile(target, Buffer.from(`${JSON.stringify(value)}\n`, "utf8"));
}

/**
 * Reads JSON. A missing file returns the fallback; a file that exists but does
 * not parse throws, so the caller can preserve it instead of inventing empty
 * data.
 */
export async function readJsonFile(file, fallback) {
  let text;
  try {
    text = await fs.readFile(file, "utf8");
  } catch (error) {
    if (error?.code === "ENOENT") return fallback;
    throw error;
  }
  try {
    return JSON.parse(text);
  } catch {
    throw new CorruptJsonError(file);
  }
}

export function byteLength(value) {
  return Buffer.byteLength(JSON.stringify(value), "utf8");
}

/** Removes files matching `pattern` inside `dir`; used for abandoned temp files. */
export async function removeMatching(dir, pattern, { maxAgeMs = 0, now = Date.now } = {}) {
  const names = await fs.readdir(dir).catch((error) => {
    if (error?.code === "ENOENT") return [];
    throw error;
  });
  let removed = 0;
  for (const name of names) {
    if (!pattern.test(name)) continue;
    const file = path.join(dir, name);
    try {
      if (maxAgeMs > 0) {
        const stat = await fs.stat(file);
        if (now() - stat.mtimeMs <= maxAgeMs) continue;
      }
      await fs.unlink(file);
      removed += 1;
    } catch {
      // Already gone or still busy; the next start sweeps it.
    }
  }
  return removed;
}

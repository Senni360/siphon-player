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
 * Writes bytes to a fresh temp file in the same directory, fsyncs, then
 * renames over the target. A crash leaves the previous file intact.
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
  } catch (error) {
    if (handle) await handle.close().catch(() => {});
    await fs.unlink(temp).catch(() => {});
    throw error;
  }
}

export async function atomicWriteJson(target, value) {
  await atomicWriteFile(target, Buffer.from(`${JSON.stringify(value)}\n`, "utf8"));
}

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
    return fallback;
  }
}

export function byteLength(value) {
  return Buffer.byteLength(JSON.stringify(value), "utf8");
}

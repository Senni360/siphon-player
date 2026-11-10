// A reader owns a cache file for as long as it is streaming it. Eviction and
// explicit removal must wait, or a phone hears silence in the middle of a bar.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { CacheStore } from "../src/cache.mjs";
import { tempDir } from "./helpers/context.mjs";

const blob = (fill, length = 40) => Buffer.alloc(length, fill);

async function sourceFile(dir, name, fill) {
  const file = path.join(dir, name);
  await fs.writeFile(file, blob(fill));
  return file;
}

test("eviction skips a file that is being streamed", async (t) => {
  const dir = await tempDir();
  const source = await tempDir();
  t.after(async () => {
    await fs.rm(dir, { recursive: true, force: true });
    await fs.rm(source, { recursive: true, force: true });
  });
  let clock = 1_000;
  const cache = new CacheStore({ dir, maxBytes: 100, now: () => clock });
  await cache.init();
  const keyA = cache.keyFor("user", "youtube:video:aaaaaaaaaaa");
  const keyB = cache.keyFor("user", "youtube:video:bbbbbbbbbbb");
  const keyC = cache.keyFor("user", "youtube:video:ccccccccccc");
  await cache.put(keyA, await sourceFile(source, "a", "a"), { contentType: "audio/mp4" });
  clock = 2_000;
  await cache.put(keyB, await sourceFile(source, "b", "b"), { contentType: "audio/mp4" });
  assert.equal(cache.pin(keyA), true);
  clock = 3_000;
  await cache.put(keyC, await sourceFile(source, "c", "c"), { contentType: "audio/mp4" });

  assert.ok(cache.get(keyA), "the pinned entry stays");
  assert.equal(cache.get(keyB), null, "the oldest unpinned entry is evicted instead");
  assert.ok(await fs.stat(cache.get(keyA).file));
  assert.equal(cache.stats().pinned, 1);

  cache.unpin(keyA);
  // A later put may now evict A, because it is the least recently played.
  clock = 4_000;
  const keyD = cache.keyFor("user", "youtube:video:ddddddddddd");
  await cache.put(keyD, await sourceFile(source, "d", "d"), { contentType: "audio/mp4" });
  assert.equal(cache.get(keyA), null);
});

test("removing a pinned entry defers the unlink until the reader lets go", async (t) => {
  const dir = await tempDir();
  const source = await tempDir();
  t.after(async () => {
    await fs.rm(dir, { recursive: true, force: true });
    await fs.rm(source, { recursive: true, force: true });
  });
  const cache = new CacheStore({ dir, maxBytes: 1024, now: () => 1 });
  await cache.init();
  const key = cache.keyFor("user", "youtube:video:eeeeeeeeeee");
  await cache.put(key, await sourceFile(source, "e", "e"), { contentType: "audio/mp4" });
  const file = cache.get(key).file;
  cache.pin(key);
  assert.equal(await cache.remove(key), true);
  assert.ok(await fs.stat(file), "the reader's file is still on disk");
  assert.equal(cache.stats().bytes, 40);

  cache.unpin(key);
  await new Promise((resolve) => setTimeout(resolve, 25));
  await assert.rejects(fs.stat(file), (error) => error.code === "ENOENT");
  assert.equal(cache.get(key), null);
  assert.equal(cache.stats().bytes, 0);
});

test("pinning survives the rename from .part to the final name", async (t) => {
  const dir = await tempDir();
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const cache = new CacheStore({ dir, maxBytes: 40, now: () => 1 });
  await cache.init();
  const key = cache.keyFor("user", "youtube:video:fffffffffff");
  const other = cache.keyFor("user", "youtube:video:ggggggggggg");
  const pending = await cache.begin(key, { contentType: "audio/mp4" });
  await fs.writeFile(pending.file, blob("f"));
  cache.progress(key, 40);
  assert.equal(cache.pin(key), true);
  await cache.complete(key);
  const entry = cache.get(key);
  assert.ok(await fs.stat(entry.file));

  const source = await tempDir();
  t.after(() => fs.rm(source, { recursive: true, force: true }));
  await cache.put(other, await sourceFile(source, "g", "g"), { contentType: "audio/mp4" });
  assert.ok(cache.get(key), "the just-finished stream is still protected");
  assert.equal(cache.get(other), null);
  cache.unpin(key);
});

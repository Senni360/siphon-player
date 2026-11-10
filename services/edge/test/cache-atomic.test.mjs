// A crashed or full disk must never leave a short file under a real cache
// name. These tests force the copy/rename paths that the happy path never
// visits on a normal filesystem.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { CacheStore } from "../src/cache.mjs";
import { tempDir } from "./helpers/context.mjs";

const blob = (fill, length = 40) => Buffer.alloc(length, fill);

function exdev(target) {
  return Object.assign(new Error("cross-device link"), { code: "EXDEV", target });
}

test("an interrupted side-filesystem copy leaves no file under the cache key", async (t) => {
  const dir = await tempDir();
  const source = await tempDir();
  t.after(async () => {
    await fs.rm(dir, { recursive: true, force: true });
    await fs.rm(source, { recursive: true, force: true });
  });
  const cache = new CacheStore({ dir, maxBytes: 1024, now: () => 1 });
  await cache.init();
  const from = path.join(source, "audio");
  await fs.writeFile(from, blob("a", 40));
  const key = cache.keyFor("user", "youtube:video:aaaaaaaaaaa");

  const realRename = fs.rename;
  const realCopyFile = fs.copyFile;
  fs.rename = async (a, b) => {
    if (String(b).includes(`${key}.m4a`)) throw exdev(b);
    return realRename(a, b);
  };
  fs.copyFile = async (a, b) => {
    // Exactly the failure mode: half the bytes reach the target, then ENOSPC.
    await fs.writeFile(b, blob("a", 20));
    throw Object.assign(new Error("no space"), { code: "ENOSPC" });
  };
  try {
    await assert.rejects(cache.put(key, from, { contentType: "audio/mp4" }), (error) => {
      assert.equal(error.code, "cache_full");
      assert.equal(error.status, 507);
      return true;
    });
  } finally {
    fs.rename = realRename;
    fs.copyFile = realCopyFile;
  }

  const names = await fs.readdir(cache.dir);
  assert.deepEqual(names.filter((name) => name.startsWith("c")), [], "no final or .part cache file remains");
  assert.equal(cache.has(key), false);

  // A restart must not adopt the fragment either.
  const reopened = new CacheStore({ dir, maxBytes: 1024, now: () => 1 });
  await reopened.init();
  assert.equal(reopened.has(key), false);
});

test("a .part file from a crash is swept and never indexed", async (t) => {
  const dir = await tempDir();
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const cache = new CacheStore({ dir, maxBytes: 1024, now: () => 1 });
  await cache.init();
  const key = cache.keyFor("user", "youtube:video:bbbbbbbbbbb");
  await fs.writeFile(path.join(cache.dir, `${key}.m4a.part`), blob("b", 40));
  await fs.writeFile(path.join(cache.dir, ".tmp-stale"), "{}");

  const reopened = new CacheStore({ dir, maxBytes: 1024, now: () => 1 });
  await reopened.init();
  assert.equal(reopened.has(key), false);
  const names = await fs.readdir(cache.dir);
  assert.deepEqual(names.filter((name) => name.includes(".part") || name.startsWith(".tmp-")), []);
});

test("a growing fetch is readable before it gets its final name", async (t) => {
  const dir = await tempDir();
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const cache = new CacheStore({ dir, maxBytes: 1024, now: () => 1 });
  await cache.init();
  const key = cache.keyFor("user", "youtube:video:ccccccccccc");
  const pending = await cache.begin(key, { contentType: "audio/mp4" });
  assert.match(pending.file, /\.m4a\.part$/);
  await fs.writeFile(pending.file, blob("c", 20));
  cache.progress(key, 20);
  assert.equal(cache.get(key), null, "nothing is final yet");
  assert.equal(cache.status(key).complete, false);
  assert.equal(cache.status(key).size, 20);

  const waited = await cache.waitForBytes(key, 10, null);
  assert.equal(waited.size, 20);
  const entry = await cache.complete(key);
  assert.equal(entry.size, 20);
  assert.match(entry.file, /\.m4a$/);
  assert.equal(cache.status(key).complete, true);
  assert.equal(await fs.readFile(entry.file).then((bytes) => bytes.length), 20);
  await assert.rejects(fs.stat(pending.file), (error) => error.code === "ENOENT");
});

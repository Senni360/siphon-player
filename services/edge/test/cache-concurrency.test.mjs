// The cache keeps its byte total in memory and evicts by unlinking files. Puts
// and evictions are async and not queued, so two puts can run `enforce` at the
// same time. These tests hold the eviction step at a barrier to make that
// interleaving happen every run instead of only on a slow disk.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { CacheStore } from "../src/cache.mjs";
import { tempDir } from "./helpers/context.mjs";

const blob = (fill, length = 40) => Buffer.alloc(length, fill);

async function sourceFiles(dir, count) {
  const files = [];
  for (let index = 0; index < count; index += 1) {
    const file = path.join(dir, `source-${index}`);
    await fs.writeFile(file, blob(index));
    files.push(file);
  }
  return files;
}

/** Runs `body` with `fs.unlink` held until the returned `release` is called. */
async function withHeldUnlink(body) {
  const real = fs.unlink;
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  let waiting = 0;
  let reached;
  const arrived = new Promise((resolve) => { reached = resolve; });
  fs.unlink = async (target) => {
    waiting += 1;
    reached();
    await gate;
    return real(target);
  };
  try {
    return await body({
      release,
      arrived,
      waiting: () => waiting,
      settle: async () => {
        // Every eviction that is going to start has started once the microtask
        // queue drains; the puts themselves are blocked at the gate.
        for (let turn = 0; turn < 5; turn += 1) await new Promise(setImmediate);
      },
    });
  } finally {
    release();
    fs.unlink = real;
  }
}

async function onDisk(cache) {
  const names = (await fs.readdir(cache.dir)).filter((name) => name.startsWith("c"));
  let bytes = 0;
  for (const name of names) bytes += (await fs.stat(path.join(cache.dir, name))).size;
  return { names, bytes };
}

test("concurrent evictions keep the byte total equal to the files that remain", async (t) => {
  const dir = await tempDir();
  const source = await tempDir();
  t.after(async () => {
    await fs.rm(dir, { recursive: true, force: true });
    await fs.rm(source, { recursive: true, force: true });
  });
  const cache = new CacheStore({ dir, maxBytes: 100, now: () => 1 });
  await cache.init();
  const files = await sourceFiles(source, 4);
  const writes = files.map((file, index) => cache.put(
    cache.keyFor("user", `youtube:video:aaaaaaaaa${index}`),
    file,
    { contentType: "audio/mp4" },
  ));
  const result = await withHeldUnlink(async ({ release, arrived, settle }) => {
    await arrived;
    await settle();
    release();
    await Promise.all(writes);
  });
  assert.equal(result, undefined);
  const disk = await onDisk(cache);
  const stats = cache.stats();
  assert.equal(stats.files, disk.names.length, "the entry count matches the disk");
  assert.equal(stats.bytes, disk.bytes, "the byte total matches the disk");
  assert.ok(stats.bytes <= 100, "the cache stays under its cap");
});

test("a put racing a removal leaves nothing behind that the cache still counts", async (t) => {
  const dir = await tempDir();
  const source = await tempDir();
  t.after(async () => {
    await fs.rm(dir, { recursive: true, force: true });
    await fs.rm(source, { recursive: true, force: true });
  });
  let clock = 1_000;
  const cache = new CacheStore({ dir, maxBytes: 100, now: () => clock });
  await cache.init();
  const files = await sourceFiles(source, 2);
  const first = cache.keyFor("user", "youtube:video:aaaaaaaaaa");
  const second = cache.keyFor("user", "youtube:video:bbbbbbbbbb");
  await cache.put(first, files[0], { contentType: "audio/mp4" });
  clock = 2_000;
  await Promise.all([
    cache.put(second, files[1], { contentType: "audio/mp4" }),
    cache.remove(first),
  ]);
  const disk = await onDisk(cache);
  const stats = cache.stats();
  assert.equal(stats.files, disk.names.length);
  assert.equal(stats.bytes, disk.bytes);
  for (const name of disk.names) {
    const key = name.slice(0, 57);
    assert.ok(cache.get(key), `the counted file ${name} is still an entry`);
  }
});

test("reads that touch an entry during an eviction never count a file twice", async (t) => {
  const dir = await tempDir();
  const source = await tempDir();
  t.after(async () => {
    await fs.rm(dir, { recursive: true, force: true });
    await fs.rm(source, { recursive: true, force: true });
  });
  let clock = 1_000;
  const cache = new CacheStore({ dir, maxBytes: 120, now: () => clock });
  await cache.init();
  const files = await sourceFiles(source, 3);
  const keys = files.map((_file, index) => cache.keyFor("user", `youtube:video:aaaaaaaaa${index}`));
  await cache.put(keys[0], files[0], { contentType: "audio/mp4" });
  clock = 2_000;
  await cache.put(keys[1], files[1], { contentType: "audio/mp4" });
  clock = 3_000;
  await Promise.all([
    cache.put(keys[2], files[2], { contentType: "audio/mp4" }),
    Promise.resolve().then(() => cache.touch(keys[0])),
    Promise.resolve().then(() => cache.touch(keys[1])),
  ]);
  const disk = await onDisk(cache);
  assert.equal(cache.stats().files, disk.names.length);
  assert.equal(cache.stats().bytes, disk.bytes);
});

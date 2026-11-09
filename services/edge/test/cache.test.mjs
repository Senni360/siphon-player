import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { CacheStore } from "../src/cache.mjs";
import { UploadStore } from "../src/uploads.mjs";
import { tempDir } from "./helpers/context.mjs";

async function sourceFile(dir, name, bytes) {
  const file = path.join(dir, name);
  await fs.writeFile(file, bytes);
  return file;
}

const blob = (fill, length = 40) => Buffer.alloc(length, fill);

test("evicts the least-recently-played entry first", async () => {
  const dir = await tempDir();
  let clock = 1_000;
  const cache = new CacheStore({ dir, maxBytes: 100, now: () => clock });
  await cache.init();
  const source = await tempDir();
  const keyA = cache.keyFor("user", "youtube:video:aaaaaaaaaaa");
  const keyB = cache.keyFor("user", "youtube:video:bbbbbbbbbbb");
  const keyC = cache.keyFor("user", "youtube:video:ccccccccccc");
  const keyD = cache.keyFor("user", "youtube:video:ddddddddddd");
  await cache.put(keyA, await sourceFile(source, "a", blob("a")), { contentType: "audio/mp4" });
  clock = 2_000;
  await cache.put(keyB, await sourceFile(source, "b", blob("b")), { contentType: "audio/mp4" });
  clock = 3_000;
  await cache.put(keyC, await sourceFile(source, "c", blob("c")), { contentType: "audio/mp4" });
  assert.equal(cache.get(keyA), null, "third put pushes the oldest out");
  clock = 4_000;
  cache.touch(keyB);
  clock = 5_000;
  await cache.put(keyD, await sourceFile(source, "d", blob("d")), { contentType: "audio/mp4" });
  assert.equal(cache.get(keyC), null, "B was played more recently than C");
  assert.notEqual(cache.get(keyB), null);
  assert.notEqual(cache.get(keyD), null);
  assert.ok(cache.stats().bytes <= 100);
});

test("last-played order survives a restart", async () => {
  const dir = await tempDir();
  let clock = 1_000;
  const cache = new CacheStore({ dir, maxBytes: 100, now: () => clock });
  await cache.init();
  const source = await tempDir();
  const keyA = cache.keyFor("user", "youtube:video:aaaaaaaaaaa");
  await cache.put(keyA, await sourceFile(source, "a", blob("a")), { contentType: "audio/mp4" });
  clock = 9_000;
  cache.touch(keyA);
  await cache.flush();
  const reopened = new CacheStore({ dir, maxBytes: 100, now: () => clock });
  await reopened.init();
  assert.equal(reopened.get(keyA).lastPlayed, 9_000);
});

test("a file larger than the whole cache is not kept", async () => {
  const dir = await tempDir();
  const cache = new CacheStore({ dir, maxBytes: 50, now: () => 1 });
  await cache.init();
  const source = await tempDir();
  const key = cache.keyFor("user", "youtube:video:aaaaaaaaaaa");
  await cache.put(key, await sourceFile(source, "big", blob("x", 60)), { contentType: "audio/mp4" });
  assert.equal(cache.get(key), null);
  assert.equal(cache.stats().bytes, 0);
});

test("cache eviction never touches uploaded files", async () => {
  const dir = await tempDir();
  const uploads = new UploadStore({ dir, userMaxBytes: 10_000, fileMaxBytes: 10_000 });
  await uploads.init();
  const uploaded = await uploads.store("user", "local-song", (async function* () {
    yield Buffer.concat([Buffer.from([0, 0, 0, 0x18]), Buffer.from("ftypM4A "), blob("u", 40)]);
  })(), { contentLength: 52 });
  const cache = new CacheStore({ dir, maxBytes: 10, now: () => 1 });
  await cache.init();
  const source = await tempDir();
  const key = cache.keyFor("user", "youtube:video:aaaaaaaaaaa");
  await cache.put(key, await sourceFile(source, "a", blob("a", 40)), { contentType: "audio/mp4" });
  assert.equal(cache.get(key), null);
  assert.equal(uploads.lookup("user", "local-song").file, uploaded.file);
  assert.ok(await fs.stat(uploaded.file));
});

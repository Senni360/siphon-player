import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { mintToken } from "../src/tokens.mjs";
import { parseRange } from "../src/range.mjs";
import { PHONE_ORIGIN, SECRET, request, startEdge, tempDir } from "./helpers/context.mjs";

const SUB = "media-user";
const CONTENT = Buffer.from("0123456789abcdef", "utf8");

async function seed(ctx, { sub = SUB, source = "youtube:video:abcdefghijk", contentType = "audio/mp4", bytes = CONTENT } = {}) {
  const key = ctx.edge.cache.keyFor(sub, source);
  const dir = await tempDir("encore-edge-seed-");
  const file = path.join(dir, "audio.bin");
  await fs.writeFile(file, bytes);
  await ctx.edge.cache.put(key, file, { contentType });
  await fs.rm(dir, { recursive: true, force: true });
  return key;
}

test("range parser covers single, suffix, open-ended and 416 cases", () => {
  assert.deepEqual(parseRange(undefined, 16), null);
  assert.deepEqual(parseRange("bytes=0-3", 16), { start: 0, end: 3 });
  assert.deepEqual(parseRange("bytes=10-", 16), { start: 10, end: 15 });
  assert.deepEqual(parseRange("bytes=10-999", 16), { start: 10, end: 15 });
  assert.deepEqual(parseRange("bytes=-4", 16), { start: 12, end: 15 });
  assert.deepEqual(parseRange("bytes=-99", 16), { start: 0, end: 15 });
  assert.deepEqual(parseRange("bytes=0-0", 16), { start: 0, end: 0 });
  for (const bad of [
    "bytes=5-2",
    "bytes=16-",
    "bytes=20-30",
    "bytes=-0",
    "bytes=0-1,3-4",
    "bytes=abc",
    "items=0-2",
    "bytes=",
    "bytes=1.5-2",
  ]) {
    assert.deepEqual(parseRange(bad, 16), { unsatisfiable: true }, bad);
  }
});

test("serves a private file with a strong ETag and no compression", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const key = await seed(ctx);
  const token = ctx.edge.media.tokenFor(SUB, key);
  const response = await request(ctx.base, `/v1/media/${key}?token=${encodeURIComponent(token)}`, {
    origin: PHONE_ORIGIN,
  });
  assert.equal(response.status, 200);
  assert.equal(Buffer.from(await response.arrayBuffer()).toString("utf8"), CONTENT.toString("utf8"));
  assert.equal(response.headers.get("accept-ranges"), "bytes");
  assert.equal(response.headers.get("content-length"), String(CONTENT.length));
  assert.equal(response.headers.get("content-encoding"), null);
  assert.match(response.headers.get("cache-control"), /private/);
  assert.match(response.headers.get("etag"), /^"[0-9a-f]+-[0-9a-f]+"$/);
  assert.equal(response.headers.get("access-control-allow-origin"), PHONE_ORIGIN);
  assert.match(response.headers.get("access-control-expose-headers"), /Content-Range/);
  assert.equal(response.headers.get("x-content-type-options"), "nosniff");
});

test("answers single, suffix and open-ended ranges exactly", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const key = await seed(ctx);
  const token = ctx.edge.media.tokenFor(SUB, key);
  const url = `/v1/media/${key}?token=${encodeURIComponent(token)}`;
  for (const [range, status, body, contentRange] of [
    ["bytes=2-5", 206, "2345", "bytes 2-5/16"],
    ["bytes=10-", 206, "abcdef", "bytes 10-15/16"],
    ["bytes=-4", 206, "cdef", "bytes 12-15/16"],
    ["bytes=10-999", 206, "abcdef", "bytes 10-15/16"],
  ]) {
    const response = await fetch(`${ctx.base}${url}`, { headers: { range } });
    assert.equal(response.status, status, range);
    assert.equal(await response.text(), body, range);
    assert.equal(response.headers.get("content-range"), contentRange, range);
    assert.equal(response.headers.get("accept-ranges"), "bytes");
  }
});

test("refuses multi-range, reversed and unsatisfiable ranges with 416", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const key = await seed(ctx);
  const token = ctx.edge.media.tokenFor(SUB, key);
  const url = `${ctx.base}/v1/media/${key}?token=${encodeURIComponent(token)}`;
  for (const range of ["bytes=0-1,3-4", "bytes=5-2", "bytes=99-", "bytes=abc", "bytes=-0"]) {
    const response = await fetch(url, { headers: { range } });
    assert.equal(response.status, 416, range);
    assert.equal(response.headers.get("content-range"), "bytes */16", range);
    assert.equal(response.headers.get("accept-ranges"), "bytes");
  }
});

test("HEAD carries the same length without a body, and If-None-Match returns 304", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const key = await seed(ctx);
  const token = ctx.edge.media.tokenFor(SUB, key);
  const url = `${ctx.base}/v1/media/${key}?token=${encodeURIComponent(token)}`;
  const head = await fetch(url, { method: "HEAD", headers: { range: "bytes=4-7" } });
  assert.equal(head.status, 206);
  assert.equal(head.headers.get("content-length"), "4");
  assert.equal(await head.text(), "");
  const first = await fetch(url);
  const etag = first.headers.get("etag");
  await first.arrayBuffer();
  const cached = await fetch(url, { headers: { "if-none-match": etag } });
  assert.equal(cached.status, 304);
  assert.equal(cached.headers.get("etag"), etag);
});

test("tokens are bound to user and key", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const key = await seed(ctx);
  const otherKey = ctx.edge.cache.keyFor(SUB, "youtube:video:zzzzzzzzzzz");

  const wrongKey = ctx.edge.media.tokenFor(SUB, otherKey);
  const wrongUser = ctx.edge.media.tokenFor("someone-else", key);
  const wrongScope = mintToken({ sub: SUB, role: "phone", scope: "sync", key }, SECRET);

  const url = (token) => `${ctx.base}/v1/media/${key}?token=${encodeURIComponent(token)}`;
  assert.equal((await fetch(url(wrongKey))).status, 403);
  assert.equal((await fetch(url(wrongUser))).status, 404);
  assert.equal((await fetch(url(wrongScope))).status, 401);
  assert.equal((await fetch(`${ctx.base}/v1/media/${key}`)).status, 401);
});

test("CORS names only the configured origin and preflight allows range", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const key = await seed(ctx);
  const token = ctx.edge.media.tokenFor(SUB, key);
  const url = `/v1/media/${key}?token=${encodeURIComponent(token)}`;
  const denied = await fetch(`${ctx.base}${url}`, { headers: { origin: "https://evil.example" } });
  assert.equal(denied.headers.get("access-control-allow-origin"), null);
  await denied.arrayBuffer();
  const preflight = await fetch(`${ctx.base}${url}`, {
    method: "OPTIONS",
    headers: { origin: PHONE_ORIGIN, "access-control-request-method": "GET", "access-control-request-headers": "range" },
  });
  assert.equal(preflight.status, 204);
  assert.equal(preflight.headers.get("access-control-allow-origin"), PHONE_ORIGIN);
  assert.match(preflight.headers.get("access-control-allow-headers"), /range/);
});

test("a missing file is a 404 and no key is echoed", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const key = ctx.edge.cache.keyFor(SUB, "youtube:video:zzzzzzzzzzz");
  const token = ctx.edge.media.tokenFor(SUB, key);
  const response = await fetch(`${ctx.base}/v1/media/${key}?token=${encodeURIComponent(token)}`);
  assert.equal(response.status, 404);
  assert.equal((await response.text()).includes(key), false);
});

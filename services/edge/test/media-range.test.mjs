// Single-range reads at the file edges, HEAD, conditional reads, and what
// happens when the disposable cache drops a file while a player is reading it.
import assert from "node:assert/strict";
import fs from "node:fs";
import fsp from "node:fs/promises";
import { PassThrough } from "node:stream";
import path from "node:path";
import test from "node:test";
import { mintToken } from "../src/tokens.mjs";
import { SECRET, request, startEdge, tempDir } from "./helpers/context.mjs";

const SUB = "range-user";
const SIZE = 100;
const HEADER = Buffer.from([0, 0, 0, 0x18, 0x66, 0x74, 0x79, 0x70, 0x4d, 0x34, 0x41, 0x20]);
const BODY = Buffer.concat([HEADER, Buffer.from(Array.from({ length: SIZE - HEADER.length }, (_, index) => index % 251))]);

function token() {
  return mintToken({ sub: SUB, role: "computer", scope: "media", ttlSeconds: 120 }, SECRET);
}

async function uploaded(t) {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const response = await request(ctx.base, "/v1/upload/range-file", { method: "PUT", token: token(), body: BODY });
  assert.equal(response.status, 201);
  const saved = await response.json();
  return { ctx, url: `${ctx.base}${saved.url}` };
}

async function ranged(url, range, init = {}) {
  const response = await fetch(url, { headers: { range, ...init } });
  const body = Buffer.from(await response.arrayBuffer());
  return { response, body };
}

test("single ranges stop exactly at the first and last byte", async (t) => {
  const { url } = await uploaded(t);
  const first = await ranged(url, "bytes=0-0");
  assert.equal(first.response.status, 206);
  assert.equal(first.response.headers.get("content-range"), `bytes 0-0/${SIZE}`);
  assert.deepEqual(first.body, BODY.subarray(0, 1));

  const last = await ranged(url, `bytes=${SIZE - 1}-${SIZE - 1}`);
  assert.equal(last.response.status, 206);
  assert.equal(last.response.headers.get("content-range"), `bytes ${SIZE - 1}-${SIZE - 1}/${SIZE}`);
  assert.deepEqual(last.body, BODY.subarray(SIZE - 1));

  const openEnded = await ranged(url, `bytes=${SIZE - 1}-`);
  assert.equal(openEnded.response.status, 206);
  assert.deepEqual(openEnded.body, BODY.subarray(SIZE - 1));

  const all = await ranged(url, "bytes=0-");
  assert.equal(all.response.status, 206);
  assert.deepEqual(all.body, BODY);

  const suffix = await ranged(url, "bytes=-1");
  assert.equal(suffix.response.status, 206);
  assert.deepEqual(suffix.body, BODY.subarray(SIZE - 1));

  const wholeSuffix = await ranged(url, `bytes=-${SIZE}`);
  assert.equal(wholeSuffix.response.status, 206);
  assert.deepEqual(wholeSuffix.body, BODY);

  const tooLongSuffix = await ranged(url, `bytes=-${SIZE + 50}`);
  assert.equal(tooLongSuffix.response.status, 206, "a suffix past the start means the whole file");
  assert.deepEqual(tooLongSuffix.body, BODY);

  const pastEnd = await ranged(url, `bytes=0-${SIZE + 500}`);
  assert.equal(pastEnd.response.status, 206);
  assert.equal(pastEnd.response.headers.get("content-range"), `bytes 0-${SIZE - 1}/${SIZE}`);
  assert.deepEqual(pastEnd.body, BODY);
});

test("a range outside the file or more than one range is a 416 with no body", async (t) => {
  const { url } = await uploaded(t);
  for (const range of [`bytes=${SIZE}-`, `bytes=${SIZE + 1}-${SIZE + 2}`, "bytes=5-3", "bytes=0-1,3-4", "bytes=-0", "bytes=--1", "items=0-1"]) {
    const { response, body } = await ranged(url, range);
    assert.equal(response.status, 416, range);
    assert.equal(response.headers.get("content-range"), `bytes */${SIZE}`, range);
    assert.equal(response.headers.get("content-length"), "0", range);
    assert.equal(body.length, 0, range);
  }
});

test("HEAD with a range sends the headers and no body", async (t) => {
  const { url } = await uploaded(t);
  const response = await fetch(url, { method: "HEAD", headers: { range: "bytes=10-19" } });
  assert.equal(response.status, 206);
  assert.equal(response.headers.get("content-range"), `bytes 10-19/${SIZE}`);
  assert.equal(response.headers.get("content-length"), "10");
  assert.equal((await response.arrayBuffer()).byteLength, 0);
});

test("a conditional read is a 304, unless a range was asked for", async (t) => {
  const { url } = await uploaded(t);
  const first = await fetch(url);
  const etag = first.headers.get("etag");
  assert.ok(etag);
  await first.arrayBuffer();
  const cached = await fetch(url, { headers: { "if-none-match": etag } });
  assert.equal(cached.status, 304);
  assert.equal((await cached.arrayBuffer()).byteLength, 0);
  const rangedRead = await fetch(url, { headers: { "if-none-match": etag, range: "bytes=0-9" } });
  assert.equal(rangedRead.status, 206, "a range is served, not answered with 304");
  assert.equal((await rangedRead.arrayBuffer()).byteLength, 10);
  const weak = await fetch(url, { headers: { "if-none-match": `W/${etag}` } });
  assert.equal(weak.status, 200);
  await weak.arrayBuffer();
});

test("an evicted cache entry is a plain 404, also for a range request", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const source = await tempDir();
  t.after(() => fsp.rm(source, { recursive: true, force: true }));
  const file = path.join(source, "cached");
  await fsp.writeFile(file, BODY);
  const key = ctx.edge.cache.keyFor(SUB, "youtube:video:aaaaaaaaaa");
  await ctx.edge.cache.put(key, file, { contentType: "audio/mp4" });
  const url = `${ctx.base}/v1/media/${key}?token=${encodeURIComponent(ctx.edge.media.tokenFor(SUB, key))}`;
  assert.equal((await fetch(url)).status, 200);
  await ctx.edge.cache.remove(key);
  const gone = await fetch(url);
  assert.equal(gone.status, 404);
  const goneRange = await fetch(url, { headers: { range: "bytes=0-9" } });
  assert.equal(goneRange.status, 404);
});

test("a cache entry evicted between the headers and the first byte never yields a short 206", {
  todo: "media.mjs resolves the entry and writes headers before opening the file; an eviction in that window closes the socket with no response at all",
}, async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const source = await tempDir();
  t.after(() => fsp.rm(source, { recursive: true, force: true }));
  const file = path.join(source, "cached");
  await fsp.writeFile(file, BODY);
  const key = ctx.edge.cache.keyFor(SUB, "youtube:video:bbbbbbbbbb");
  await ctx.edge.cache.put(key, file, { contentType: "audio/mp4" });
  const url = `${ctx.base}/v1/media/${key}?token=${encodeURIComponent(ctx.edge.media.tokenFor(SUB, key))}`;

  const real = fs.createReadStream;
  let reached;
  const arrived = new Promise((resolve) => { reached = resolve; });
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  fs.createReadStream = (target, options) => {
    reached();
    const out = new PassThrough();
    gate.then(() => {
      const inner = real(target, options);
      inner.on("error", (error) => out.destroy(error));
      inner.pipe(out);
    });
    return out;
  };
  let outcome;
  try {
    const reading = fetch(url, { headers: { range: "bytes=0-99" } }).then(async (response) => ({
      status: response.status,
      body: Buffer.from(await response.arrayBuffer()),
    })).catch((error) => ({ error }));
    await arrived;
    await ctx.edge.cache.remove(key);
    release();
    outcome = await reading;
  } finally {
    release();
    fs.createReadStream = real;
  }
  assert.equal(outcome.error, undefined, `the request failed at the transport layer: ${outcome.error?.cause?.code ?? outcome.error?.message}`);
  assert.equal(outcome.status, 206);
  assert.equal(outcome.body.length, SIZE, "a 206 must carry the bytes it promised");
});

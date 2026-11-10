import assert from "node:assert/strict";
import fs from "node:fs/promises";
import test from "node:test";
import { mintToken } from "../src/tokens.mjs";
import { SECRET, request, startEdge, tempDir } from "./helpers/context.mjs";

const SUB = "computer-user";
const MP4 = Buffer.concat([Buffer.from([0, 0, 0, 0x18]), Buffer.from("ftypM4A "), Buffer.alloc(64, 0x41)]);
const MP3 = Buffer.concat([Buffer.from("ID3"), Buffer.alloc(64, 0x21)]);

function computerToken(extra = {}) {
  return mintToken({ sub: SUB, role: "computer", scope: "media", ttlSeconds: 120, ...extra }, SECRET);
}

async function upload(ctx, key, bytes, token = computerToken()) {
  return request(ctx.base, `/v1/upload/${encodeURIComponent(key)}`, { method: "PUT", token, body: bytes });
}

test("streams a valid audio file in and serves it back with a media token", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const response = await upload(ctx, "local-song", MP4);
  assert.equal(response.status, 201);
  const saved = await response.json();
  assert.equal(saved.key, "local-song");
  assert.equal(saved.size, MP4.length);
  assert.equal(saved.contentType, "audio/mp4");
  assert.match(saved.etag, /^"[0-9a-f]+-[0-9a-f]+"$/);
  const media = await fetch(`${ctx.base}${saved.url}`);
  assert.equal(media.status, 200);
  assert.equal(media.headers.get("content-type"), "audio/mp4");
  assert.deepEqual(Buffer.from(await media.arrayBuffer()), MP4);
  const ranged = await fetch(`${ctx.base}${saved.url}`, { headers: { range: "bytes=0-11" } });
  assert.equal(ranged.status, 206);
  assert.equal(Buffer.from(await ranged.arrayBuffer()).length, 12);
});

test("accepts MP3 bytes as well as MP4", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const response = await upload(ctx, "local-mp3", MP3);
  assert.equal(response.status, 201);
  assert.equal((await response.json()).contentType, "audio/mpeg");
  assert.equal(ctx.edge.uploads.lookup(SUB, "local-mp3").contentType, "audio/mpeg");
});

test("rejects bytes that do not look like audio and leaves no file", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const response = await upload(ctx, "not-audio", Buffer.from("this is plainly text, not audio"));
  assert.equal(response.status, 415);
  assert.equal((await response.json()).error, "not_audio");
  assert.equal(ctx.edge.uploads.lookup(SUB, "not-audio"), null);
  const mediaToken = ctx.edge.media.tokenFor(SUB, "not-audio");
  assert.equal((await fetch(`${ctx.base}/v1/media/not-audio?token=${encodeURIComponent(mediaToken)}`)).status, 404);
});

test("a fetched YouTube song resolves from the upload before any tool runs", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  await upload(ctx, "yt-abcdefghijk", MP4);
  const answer = await request(ctx.base, "/v1/resolve", {
    method: "POST",
    token: computerToken(),
    body: { source: "youtube:video:abcdefghijk" },
  });
  assert.equal(answer.status, 200);
  const record = await answer.json();
  assert.equal(record.key, "yt-abcdefghijk");
  assert.equal(record.cached, true);
  assert.equal(record.streaming, false);
  assert.equal(record.size, MP4.length);
  const media = await fetch(`${ctx.base}${record.url}`);
  assert.equal(media.status, 200);
  assert.deepEqual(Buffer.from(await media.arrayBuffer()), MP4);

  // A playlist local file resolves the same way, and a missing one is final.
  await upload(ctx, "local-resolve", MP3);
  const playlist = await request(ctx.base, "/v1/resolve", {
    method: "POST",
    token: computerToken(),
    body: { source: "upload:local-resolve" },
  });
  assert.equal(playlist.status, 200);
  assert.equal((await playlist.json()).contentType, "audio/mpeg");
  const missing = await request(ctx.base, "/v1/resolve", {
    method: "POST",
    token: computerToken(),
    body: { source: "upload:local-missing" },
  });
  assert.equal(missing.status, 404);
});

test("rejects an empty body", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const response = await upload(ctx, "empty", Buffer.alloc(0));
  assert.equal(response.status, 400);
});

test("enforces the file and per-user caps", async (t) => {
  const ctx = await startEdge({ env: { EDGE_UPLOAD_FILE_MAX_BYTES: "512", EDGE_UPLOAD_USER_MAX_BYTES: "900" } });
  t.after(() => ctx.close());
  const tooBig = await upload(ctx, "big", Buffer.concat([MP4, Buffer.alloc(600, 1)]));
  assert.equal(tooBig.status, 413);
  const first = await upload(ctx, "first", Buffer.concat([MP4, Buffer.alloc(400, 1)]));
  assert.equal(first.status, 201);
  const second = await upload(ctx, "second", Buffer.concat([MP4, Buffer.alloc(400, 1)]));
  assert.equal(second.status, 413);
  assert.equal((await second.json()).error, "user_limit_reached");
});

test("only the computer role may upload, and a bound key must match", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const phone = mintToken({ sub: SUB, role: "phone", scope: "media", ttlSeconds: 120 }, SECRET);
  assert.equal((await upload(ctx, "phone-file", MP4, phone)).status, 401);
  const wrongKey = mintToken({ sub: SUB, role: "computer", scope: "media", key: "another-key", ttlSeconds: 120 }, SECRET);
  assert.equal((await upload(ctx, "bound-file", MP4, wrongKey)).status, 403);
  const rightKey = mintToken({ sub: SUB, role: "computer", scope: "media", key: "bound-file", ttlSeconds: 120 }, SECRET);
  assert.equal((await upload(ctx, "bound-file", MP4, rightKey)).status, 201);
});

test("rejects path traversal and shell-shaped keys", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  for (const pathname of [
    "/v1/upload/..%2Fevil",
    "/v1/upload/a%5Cb",
    "/v1/upload/%2e%2e",
    "/v1/upload/-option",
    "/v1/upload/.",
    "/v1/upload/",
    "/v1/upload/a%2Fb",
  ]) {
    const response = await fetch(`${ctx.base}${pathname}`, {
      method: "PUT",
      headers: { authorization: `Bearer ${computerToken()}` },
      body: MP4,
    });
    assert.ok([400, 404].includes(response.status), `${pathname} -> ${response.status}`);
  }
});

test("an uploaded file is stored apart from a full cache and is never evicted", async (t) => {
  const ctx = await startEdge({ env: { EDGE_CACHE_MAX_BYTES: "120" } });
  t.after(() => ctx.close());
  const response = await upload(ctx, "kept-file", MP4);
  assert.equal(response.status, 201);
  const { url } = await response.json();
  const tmp = await tempDir();
  t.after(() => fs.rm(tmp, { recursive: true, force: true }));
  for (let index = 0; index < 4; index += 1) {
    const file = `${tmp}/f${index}`;
    await fs.writeFile(file, Buffer.alloc(40, index));
    const key = ctx.edge.cache.keyFor(SUB, `youtube:video:abcdefghij${index}`);
    await ctx.edge.cache.put(key, file, { contentType: "audio/mp4" });
  }
  const media = await fetch(`${ctx.base}${url}`);
  assert.equal(media.status, 200);
  assert.equal((await media.arrayBuffer()).byteLength, MP4.length);
  assert.ok(ctx.edge.uploads.lookup(SUB, "kept-file"));
});

// Uploads are streamed to a temp file and moved into place, with a per-user
// byte cap kept in memory. These tests press on the cap edges, a half-finished
// body and two requests touching the same account at once.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import test from "node:test";
import { mintToken } from "../src/tokens.mjs";
import { UploadStore } from "../src/uploads.mjs";
import { SECRET, request, startEdge, tempDir } from "./helpers/context.mjs";

const SUB = "computer-user";
const HEADER = Buffer.from([0, 0, 0, 0x18, 0x66, 0x74, 0x79, 0x70, 0x4d, 0x34, 0x41, 0x20]); // ftypM4A

/** A valid MP4 of exactly `length` bytes (the header is 12). */
function audio(length) {
  assert.ok(length >= 12);
  return Buffer.concat([HEADER, Buffer.alloc(length - 12, 0x41)]);
}

function computerToken() {
  return mintToken({ sub: SUB, role: "computer", scope: "media", ttlSeconds: 120 }, SECRET);
}

async function upload(ctx, key, bytes, token = computerToken()) {
  return request(ctx.base, `/v1/upload/${encodeURIComponent(key)}`, { method: "PUT", token, body: bytes });
}

async function tempFiles(ctx) {
  return fs.readdir(path.join(ctx.dir, "uploads", ".tmp")).catch(() => []);
}

test("the file and user caps accept a body exactly at the boundary", async (t) => {
  const ctx = await startEdge({ env: { EDGE_UPLOAD_FILE_MAX_BYTES: "512", EDGE_UPLOAD_USER_MAX_BYTES: "900" } });
  t.after(() => ctx.close());
  assert.equal((await upload(ctx, "exact-file", audio(512))).status, 201);
  assert.equal((await upload(ctx, "exact-user", audio(388))).status, 201);
  assert.equal(ctx.edge.uploads.usage(SUB), 900);
  // One more byte for this account is refused, and nothing is written.
  const refused = await upload(ctx, "over-user", audio(12));
  assert.equal(refused.status, 413);
  assert.equal((await refused.json()).error, "user_limit_reached");
  assert.equal(ctx.edge.uploads.usage(SUB), 900);
  // A replacement does not add to the account, so it still fits exactly.
  assert.equal((await upload(ctx, "exact-file", audio(512))).status, 201);
  assert.equal(ctx.edge.uploads.usage(SUB), 900);
  // One byte past the per-file cap is refused before anything is kept.
  const overFile = await upload(ctx, "over-file", audio(513));
  assert.equal(overFile.status, 413);
  assert.equal((await overFile.json()).error, "file_too_large");
  assert.equal(ctx.edge.uploads.lookup(SUB, "over-file"), null);
});

test("a body that stops mid-upload leaves no temp file, entry or usage", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const port = Number(new URL(ctx.base).port);
  await new Promise((resolve) => {
    const socket = net.connect(port, "127.0.0.1", () => {
      socket.write(
        `PUT /v1/upload/half HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer ${computerToken()}\r\n` +
        "Content-Length: 4000\r\nConnection: close\r\n\r\n",
      );
      socket.write(audio(400), () => {
        // The server has a temp file and is waiting for the rest.
        setTimeout(() => { socket.destroy(); resolve(); }, 40);
      });
    });
    socket.on("error", () => resolve());
  });
  for (let attempt = 0; attempt < 40 && (await tempFiles(ctx)).length > 0; attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  assert.deepEqual(await tempFiles(ctx), []);
  assert.equal(ctx.edge.uploads.lookup(SUB, "half"), null);
  assert.equal(ctx.edge.uploads.usage(SUB), 0);
});

test("two uploads of one key in flight at once settle on one file and one size", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const [first, second] = await Promise.all([
    upload(ctx, "same-key", audio(120)),
    upload(ctx, "same-key", audio(240)),
  ]);
  assert.equal(first.status, 201);
  assert.equal(second.status, 201);
  const saved = ctx.edge.uploads.lookup(SUB, "same-key");
  assert.ok([120, 240].includes(saved.size), `one whole upload is kept, not a mix (${saved.size})`);
  assert.equal(ctx.edge.uploads.usage(SUB), saved.size, "the account is charged for one file");
  const owner = path.join(ctx.edge.uploads.dir, saved.owner);
  assert.equal((await fs.readdir(owner)).length, 1);
});

test("a removal during another upload does not inflate the account's usage", {
  todo: "uploads.mjs #store computes projected usage before its final awaits, so a concurrent remove's subtraction is overwritten",
}, async (t) => {
  const dir = await tempDir();
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const uploads = new UploadStore({ dir, userMaxBytes: 100_000, fileMaxBytes: 100_000 });
  await uploads.init();
  const stream = (length) => (async function* () { yield audio(length); })();
  await uploads.store(SUB, "alpha", stream(100), { contentLength: 100 });

  const real = fs.rename;
  let reached;
  const arrived = new Promise((resolve) => { reached = resolve; });
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  fs.rename = async (from, to) => { reached(); await gate; return real(from, to); };
  try {
    const pending = uploads.store(SUB, "beta", stream(100), { contentLength: 100 });
    await arrived;
    // The store has already worked out what the account will hold, and is
    // waiting to move the file. Take another song away in that window.
    assert.equal(await uploads.remove(SUB, "alpha"), true);
    release();
    await pending;
  } finally {
    release();
    fs.rename = real;
  }
  const saved = uploads.lookup(SUB, "beta");
  assert.ok(saved);
  assert.equal(uploads.usage(SUB), saved.size, "the account holds exactly the song that remains");
  assert.equal(uploads.stats().bytes, saved.size);
});

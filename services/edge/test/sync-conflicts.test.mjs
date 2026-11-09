// Revision conflicts, merge order and the per-user byte boundary of the sync
// store, plus a client that disappears halfway through a PUT.
import assert from "node:assert/strict";
import net from "node:net";
import test from "node:test";
import { mintToken } from "../src/tokens.mjs";
import { mergeMatches, SyncStore } from "../src/sync.mjs";
import fs from "node:fs/promises";
import { SECRET, request, startEdge, tempDir } from "./helpers/context.mjs";

const SUB = "sync-conflict-user";
const token = (sub = SUB) => mintToken({ sub, role: "phone", scope: "sync", ttlSeconds: 120 }, SECRET);

const match = (overrides = {}) => ({
  source: "youtube",
  sourceUri: "youtube:video:abcdefghijk",
  confidence: 0.9,
  date: "2026-01-01T00:00:00.000Z",
  manual: false,
  ...overrides,
});

test("two writers holding one revision: exactly one wins", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const put = (data) => request(ctx.base, "/v1/sync/playlists", {
    method: "PUT", token: token(), body: { data, revision: 0 },
  });
  const [one, two] = await Promise.all([put({ writer: "one" }), put({ writer: "two" })]);
  const statuses = [one.status, two.status].sort();
  assert.deepEqual(statuses, [200, 409]);
  const loaded = await (await request(ctx.base, "/v1/sync/playlists", { token: token() })).json();
  assert.equal(loaded.revision, 1);
  const winner = one.status === 200 ? "one" : "two";
  assert.equal(loaded.data.writer, winner, "the stored document is the one that won");
});

test("two unconditional writers both land, one after the other", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const put = (data) => request(ctx.base, "/v1/sync/queue", { method: "PUT", token: token(), body: { data } });
  const [one, two] = await Promise.all([put({ writer: "one" }), put({ writer: "two" })]);
  assert.deepEqual([one.status, two.status], [200, 200]);
  const loaded = await (await request(ctx.base, "/v1/sync/queue", { token: token() })).json();
  assert.equal(loaded.revision, 2, "neither write was lost");
  assert.equal(loaded.data.writer, "two", "the later call is the last write");
});

test("a refused revision leaves the document and its revision untouched", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const first = await request(ctx.base, "/v1/sync/playlists", { method: "PUT", token: token(), body: { data: { keep: true } } });
  assert.equal(first.status, 200);
  for (const revision of [-1, 1.5, "1", null]) {
    const refused = await request(ctx.base, "/v1/sync/playlists", {
      method: "PUT", token: token(), body: { data: { keep: false }, revision },
    });
    assert.equal(refused.status, 400, String(revision));
  }
  for (const revision of [0, 2, 99]) {
    const stale = await request(ctx.base, "/v1/sync/playlists", {
      method: "PUT", token: token(), body: { data: { keep: false }, revision },
    });
    assert.equal(stale.status, 409, String(revision));
  }
  const loaded = await (await request(ctx.base, "/v1/sync/playlists", { token: token() })).json();
  assert.equal(loaded.revision, 1);
  assert.deepEqual(loaded.data, { keep: true });
});

test("the per-user byte limit accepts the exact size and refuses the next byte", async (t) => {
  const dir = await tempDir();
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const now = 1_000_000;
  const record = (data) => Buffer.byteLength(JSON.stringify({
    v: 1, doc: "playlists", revision: 1, updatedAt: new Date(now).toISOString(), data,
  }));
  const data = { note: "x".repeat(200) };
  const exact = record(data);
  const store = new SyncStore({ dir, userMaxBytes: exact, now: () => now });
  const saved = await store.put(SUB, "playlists", { data });
  assert.equal(saved.revision, 1);
  await assert.rejects(store.put(SUB, "playlists", { data: { note: "x".repeat(201) } }), (error) => error.code === "user_limit_reached");
  const loaded = await store.get(SUB, "playlists");
  assert.equal(loaded.revision, 1, "a refused write does not advance the revision");
  assert.equal(loaded.data.note, "x".repeat(200));
});

test("a manual match survives an automatic write that races it", async () => {
  const uri = "spotify:track:race";
  const stored = { [uri]: match({ manual: false, confidence: 0.5 }) };
  const manual = { [uri]: match({ manual: true, sourceUri: "soundcloud:artist/song", confidence: 0.4 }) };
  const automatic = { [uri]: match({ manual: false, confidence: 0.99, date: "2027-01-01T00:00:00.000Z" }) };
  for (const order of [[manual, automatic], [automatic, manual]]) {
    let data = { ...stored };
    for (const incoming of order) data = mergeMatches(data, incoming).matches;
    assert.equal(data[uri].manual, true, "a hand choice is never taken back by an automatic one");
  }
});

test("a client that vanishes mid-PUT stores nothing", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const port = Number(new URL(ctx.base).port);
  const body = JSON.stringify({ data: { partial: "x".repeat(2000) } });
  await new Promise((resolve) => {
    const socket = net.connect(port, "127.0.0.1", () => {
      socket.write(
        `PUT /v1/sync/playlists HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer ${token()}\r\n` +
        `Content-Type: application/json\r\nContent-Length: ${body.length}\r\nConnection: close\r\n\r\n`,
      );
      socket.write(body.slice(0, 40), () => setTimeout(() => { socket.destroy(); resolve(); }, 40));
    });
    socket.on("error", () => resolve());
  });
  await new Promise((resolve) => setTimeout(resolve, 150));
  const loaded = await (await request(ctx.base, "/v1/sync/playlists", { token: token() })).json();
  assert.equal(loaded.revision, 0);
  assert.equal(loaded.data, null);
  assert.equal((await fetch(`${ctx.base}/health`)).status, 200);
});

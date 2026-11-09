import assert from "node:assert/strict";
import test from "node:test";
import { mintToken } from "../src/tokens.mjs";
import { SECRET, request, startEdge, tempDir } from "./helpers/context.mjs";
import { SyncStore } from "../src/sync.mjs";

const SUB = "sync-user";
const token = () => mintToken({ sub: SUB, role: "phone", scope: "sync", ttlSeconds: 120 }, SECRET);

function match(overrides = {}) {
  return {
    source: "youtube",
    sourceUri: "youtube:video:abcdefghijk",
    confidence: 0.9,
    date: "2026-01-01T00:00:00.000Z",
    manual: false,
    ...overrides,
  };
}

test("stores and reads a mixed playlists snapshot", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const snapshot = {
    playlists: [
      { id: "p1", name: "Mix", spotify: true, tracks: ["spotify:track:a"] },
      { id: "p2", name: "Local", spotify: false, mixed: true, tracks: ["youtube:video:abcdefghijk", "soundcloud:artist/song"] },
    ],
    revision: 7,
  };
  const put = await request(ctx.base, "/v1/sync/playlists", {
    method: "PUT",
    token: token(),
    body: { data: snapshot },
  });
  assert.equal(put.status, 200);
  const saved = await put.json();
  assert.equal(saved.revision, 1);
  const get = await request(ctx.base, "/v1/sync/playlists", { token: token() });
  assert.equal(get.status, 200);
  const loaded = await get.json();
  assert.deepEqual(loaded.data, snapshot);
  assert.equal(loaded.revision, 1);
  assert.equal(get.headers.get("etag"), "\"1\"");
});

test("queue records round-trip and are per user", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const queue = { tracks: ["spotify:track:a", "spotify:track:b"], position: 1, updatedAt: "2026-01-01T00:00:00Z" };
  assert.equal((await request(ctx.base, "/v1/sync/queue", { method: "PUT", token: token(), body: { data: queue } })).status, 200);
  const other = mintToken({ sub: "another-user", role: "computer", scope: "sync", ttlSeconds: 120 }, SECRET);
  const empty = await (await request(ctx.base, "/v1/sync/queue", { token: other })).json();
  assert.equal(empty.data, null);
  assert.equal(empty.revision, 0);
  const loaded = await (await request(ctx.base, "/v1/sync/queue", { token: token() })).json();
  assert.deepEqual(loaded.data, queue);
});

test("revision conflicts are refused, and a newer revision is accepted", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  await request(ctx.base, "/v1/sync/playlists", { method: "PUT", token: token(), body: { data: { n: 1 } } });
  const stale = await request(ctx.base, "/v1/sync/playlists", {
    method: "PUT",
    token: token(),
    body: { data: { n: 2 }, revision: 0 },
  });
  assert.equal(stale.status, 409);
  const fresh = await request(ctx.base, "/v1/sync/playlists", {
    method: "PUT",
    token: token(),
    body: { data: { n: 3 }, revision: 1 },
  });
  assert.equal(fresh.status, 200);
  assert.equal((await fresh.json()).data.n, 3);
});

test("a per-user size limit is enforced", async (t) => {
  const ctx = await startEdge({ env: { EDGE_SYNC_USER_MAX_BYTES: "2048" } });
  t.after(() => ctx.close());
  const small = await request(ctx.base, "/v1/sync/playlists", { method: "PUT", token: token(), body: { data: { a: 1 } } });
  assert.equal(small.status, 200);
  const huge = await request(ctx.base, "/v1/sync/playlists", {
    method: "PUT",
    token: token(),
    body: { data: { blob: "x".repeat(4096) } },
  });
  assert.equal(huge.status, 413);
  assert.equal((await huge.json()).error, "user_limit_reached");
});

test("documents survive a fresh store instance", async () => {
  const dir = await tempDir();
  const first = new SyncStore({ dir, now: () => 1_000 });
  await first.put(SUB, "queue", { data: { position: 42 } });
  const second = new SyncStore({ dir, now: () => 2_000 });
  const loaded = await second.get(SUB, "queue");
  assert.equal(loaded.data.position, 42);
  assert.equal(loaded.revision, 1);
});

test("a hand-chosen match always outranks an automatic one", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const uri = "spotify:track:one";
  const put = (data) => request(ctx.base, "/v1/sync/matches", { method: "PUT", token: token(), body: { data } });
  const get = async () => (await (await request(ctx.base, "/v1/sync/matches", { token: token() })).json()).data;

  assert.equal((await put({ [uri]: match() })).status, 200);
  assert.equal((await get())[uri].manual, false);

  assert.equal((await put({ [uri]: match({ manual: true, confidence: 0.4, sourceUri: "soundcloud:artist/song", date: "2025-01-01T00:00:00Z" }) })).status, 200);
  const chosen = (await get())[uri];
  assert.equal(chosen.manual, true);
  assert.equal(chosen.sourceUri, "soundcloud:artist/song");

  // A newer, more confident automatic answer cannot take it back.
  assert.equal((await put({ [uri]: match({ confidence: 0.99, date: "2027-01-01T00:00:00Z" }) })).status, 200);
  const stillChosen = (await get())[uri];
  assert.equal(stillChosen.manual, true);
  assert.equal(stillChosen.confidence, 0.4);

  // A newer hand choice wins over the older hand choice.
  assert.equal((await put({ [uri]: match({ manual: true, confidence: 0.6, date: "2027-06-01T00:00:00Z" }) })).status, 200);
  assert.equal((await get())[uri].confidence, 0.6);
});

test("automatic entries can be removed; hand-chosen ones cannot", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const put = (data, remove) => request(ctx.base, "/v1/sync/matches", {
    method: "PUT",
    token: token(),
    body: { data, remove },
  });
  const get = async () => (await (await request(ctx.base, "/v1/sync/matches", { token: token() })).json()).data;
  await put({
    "spotify:track:auto": match(),
    "spotify:track:hand": match({ manual: true, sourceUri: "soundcloud:artist/hand" }),
  });
  const removed = await put({}, ["spotify:track:auto", "spotify:track:hand"]);
  const body = await removed.json();
  assert.deepEqual(body.kept, ["spotify:track:hand"]);
  const after = await get();
  assert.equal(after["spotify:track:auto"], undefined);
  assert.equal(after["spotify:track:hand"].manual, true);
});

test("rejects malformed match entries and bad documents", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  for (const bad of [
    { "spotify:track:x": match({ source: "piracy" }) },
    { "spotify:track:x": match({ confidence: 2 }) },
    { "spotify:track:x": match({ manual: "yes" }) },
    { "not-spotify": match() },
  ]) {
    const response = await request(ctx.base, "/v1/sync/matches", { method: "PUT", token: token(), body: { data: bad } });
    assert.equal(response.status, 400, JSON.stringify(bad));
  }
  assert.equal((await request(ctx.base, "/v1/sync/unknown", { token: token() })).status, 404);
  assert.equal((await request(ctx.base, "/v1/sync/playlists", { token: mintToken({ sub: SUB, role: "phone", scope: "live" }, SECRET) })).status, 401);
});

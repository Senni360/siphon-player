import assert from "node:assert/strict";
import test from "node:test";
import { createLogger, redact, safeUrl } from "../src/log.mjs";
import { ADMIN, SECRET, request, startEdge } from "./helpers/context.mjs";
import { mintToken } from "../src/tokens.mjs";

test("health is public and minimal", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const response = await fetch(`${ctx.base}/health`);
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { v: 1, ok: true });
  const head = await fetch(`${ctx.base}/health`, { method: "HEAD" });
  assert.equal(head.status, 200);
  assert.equal(head.headers.get("content-length"), String(Buffer.byteLength("{\"v\":1,\"ok\":true}")));
});

test("status needs the admin token and reports only safe numbers", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  assert.equal((await fetch(`${ctx.base}/status`)).status, 401);
  assert.equal((await fetch(`${ctx.base}/status`, { headers: { authorization: "Bearer wrong" } })).status, 401);
  const response = await fetch(`${ctx.base}/status`, { headers: { authorization: `Bearer ${ADMIN}` } });
  assert.equal(response.status, 200);
  const view = await response.json();
  assert.equal(view.v, 1);
  assert.equal(typeof view.uptimeSeconds, "number");
  assert.ok(view.disk === null || (typeof view.disk.freeBytes === "number" && typeof view.disk.totalBytes === "number"));
  assert.deepEqual(Object.keys(view.cache).sort(), ["bytes", "capBytes", "files", "pending", "pinned"]);
  assert.deepEqual(Object.keys(view.uploads).sort(), ["bytes", "files"]);
  assert.deepEqual(Object.keys(view.queue).sort(), ["jobs", "queued", "running", "warmQueued"]);
  assert.deepEqual(Object.keys(view.connections).sort(), ["bufferBytes", "computer", "epoch", "phone", "rooms", "total"]);
  assert.equal(typeof view.sync.users, "number");
  const text = JSON.stringify(view);
  assert.equal(text.includes(ADMIN), false);
  assert.equal(text.includes(ctx.dir), false);
});

test("health and status routes reject the wrong methods", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  assert.equal((await fetch(`${ctx.base}/health`, { method: "POST" })).status, 405);
  assert.equal((await request(ctx.base, "/v1/sync/playlists", { method: "DELETE", token: syncToken() })).status, 405);
  assert.equal((await fetch(`${ctx.base}/nope`)).status, 404);
});

test("malformed JSON and an oversized body are refused", async (t) => {
  const ctx = await startEdge({ env: { EDGE_MAX_JSON_BYTES: "256" } });
  t.after(() => ctx.close());
  const bad = await fetch(`${ctx.base}/v1/sync/playlists`, {
    method: "PUT",
    headers: { authorization: `Bearer ${syncToken()}`, "content-type": "application/json" },
    body: "{",
  });
  assert.equal(bad.status, 400);
  assert.equal((await bad.json()).error, "invalid_json");
  const huge = await fetch(`${ctx.base}/v1/sync/playlists`, {
    method: "PUT",
    headers: { authorization: `Bearer ${syncToken()}`, "content-type": "application/json" },
    body: JSON.stringify({ data: { x: "y".repeat(1024) } }),
  });
  assert.equal(huge.status, 413);
});

test("no log line contains a token, a query string or a media key", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const token = mintToken({ sub: "log-user", role: "phone", scope: "media", ttlSeconds: 120 }, SECRET);
  await fetch(`${ctx.base}/health?token=${encodeURIComponent(token)}&private=link`);
  await fetch(`${ctx.base}/status?token=${ADMIN}`);
  const key = ctx.edge.cache.keyFor("log-user", "youtube:video:abcdefghijk");
  await fetch(`${ctx.base}/v1/media/${key}?token=${encodeURIComponent(token)}`);
  await fetch(`${ctx.base}/v1/sync/matches?token=${encodeURIComponent(token)}`);
  assert.ok(ctx.logs.length > 0);
  const joined = ctx.logs.join("\n");
  for (const forbidden of [token, ADMIN, key, "token=", "private=link", "abcdefghijk"]) {
    assert.equal(joined.includes(forbidden), false, forbidden);
  }
  assert.match(joined, /"route":"\/v1\/media\/:key"/);
});

test("logger scrubbers keep URLs and text safe as a last resort", () => {
  assert.equal(safeUrl("https://example.test/a/b?token=secret#frag"), "https://example.test/a/b");
  assert.equal(safeUrl("not a url"), "");
  assert.equal(redact("GET /x?token=abc&other=1"), "GET /x?token=<redacted>&other=1");
  assert.equal(redact("Authorization: Bearer abc.def"), "Authorization: Bearer <redacted>");
  const lines = [];
  const logger = createLogger({ level: "debug", sink: { log: (line) => lines.push(line) } });
  logger.info("event", { note: "https://x.test/y?token=abc", count: 2, secret: undefined });
  assert.equal(lines[0].includes("abc"), false);
  assert.equal(lines[0].includes("\"count\":2"), true);
});

test("close is idempotent and stops accepting requests", async () => {
  const ctx = await startEdge();
  await ctx.edge.close();
  await ctx.edge.close();
  await assert.rejects(fetch(`${ctx.base}/health`));
  await ctx.close();
});

function syncToken() {
  return mintToken({ sub: "log-user", role: "phone", scope: "sync", ttlSeconds: 120 }, SECRET);
}

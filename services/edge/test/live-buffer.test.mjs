// The live replay buffer is capped by bytes, not only by message count: a
// 420 MB container cannot hold 500 x 64 KiB of state per room.
import assert from "node:assert/strict";
import test from "node:test";
import { mintToken } from "../src/tokens.mjs";
import { ADMIN, SECRET, startEdge } from "./helpers/context.mjs";
import { openClient } from "./helpers/ws.mjs";

const SUB = "buffer-user";
const token = (role) => mintToken({ sub: SUB, role, scope: "live", ttlSeconds: 120 }, SECRET);
const liveUrl = (base, role, since) => `${base}/v1/live?token=${encodeURIComponent(token(role))}${since === undefined ? "" : `&since=${since}`}`;
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test("a room's replay buffer stays within its byte cap", async (t) => {
  const cap = 256 * 1024;
  const ctx = await startEdge({
    env: {
      EDGE_LIVE_BUFFER_BYTES: String(cap),
      EDGE_LIVE_BUFFER_MESSAGES: "500",
      EDGE_LIVE_ENVELOPE_MAX_BYTES: String(64 * 1024),
    },
  });
  t.after(() => ctx.close());
  const computer = await openClient(liveUrl(ctx.wsBase, "computer"));
  t.after(() => computer.close());
  await computer.next((m) => m.t === "hello");

  const body = "x".repeat(32 * 1024);
  for (let index = 0; index < 40; index += 1) {
    computer.send({ v: 1, t: "state", id: `state-${index}`, playing: true, blob: body });
  }
  await wait(400);

  const status = await fetch(`${ctx.base}/status`, { headers: { authorization: `Bearer ${ADMIN}` } });
  const view = await status.json();
  assert.ok(view.connections.bufferBytes <= cap, `buffer was ${view.connections.bufferBytes} bytes`);
  assert.ok(view.connections.bufferBytes > 0);

  const phone = await openClient(liveUrl(ctx.wsBase, "phone", 0));
  t.after(() => phone.close());
  const hello = await phone.next((m) => m.t === "hello");
  assert.equal(hello.reset, true, "the oldest state was dropped, so a reload is needed");
  assert.ok(hello.replayed < 40);
  assert.equal(hello.state.body.id, "state-39");
});

test("a state envelope larger than the per-message cap is refused", async (t) => {
  const ctx = await startEdge({ env: { EDGE_LIVE_ENVELOPE_MAX_BYTES: "2048" } });
  t.after(() => ctx.close());
  const phone = await openClient(liveUrl(ctx.wsBase, "phone"));
  t.after(() => phone.close());
  await phone.next((m) => m.t === "hello");
  phone.send({ v: 1, t: "state", id: "too-big", blob: "y".repeat(4096) });
  const reply = await phone.next((m) => m.t === "reply" && m.id === "too-big");
  assert.equal(reply.ok, false);
  assert.equal(reply.error, "invalid_envelope");
});

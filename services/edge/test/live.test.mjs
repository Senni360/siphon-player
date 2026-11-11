import assert from "node:assert/strict";
import test from "node:test";
import WebSocket from "ws";
import { mintToken } from "../src/tokens.mjs";
import { ADMIN, PHONE_ORIGIN, SECRET, startEdge } from "./helpers/context.mjs";
import { closedEvent, openClient } from "./helpers/ws.mjs";

function token(scope, role, sub = "listener") {
  return mintToken({ sub, role, scope, ttlSeconds: 120 }, SECRET);
}

function liveUrl(base, value, since) {
  return `${base}/v1/live?token=${encodeURIComponent(value)}${since === undefined ? "" : `&since=${since}`}`;
}

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test("relays a command to the other role, deduplicates its id and keeps sequence numbers", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const phone = await openClient(liveUrl(ctx.wsBase, token("live", "phone")), { headers: { Origin: PHONE_ORIGIN } });
  const computer = await openClient(liveUrl(ctx.wsBase, token("live", "computer")), { headers: { Origin: PHONE_ORIGIN } });
  t.after(async () => {
    await phone.close();
    await computer.close();
  });
  assert.equal((await phone.next((m) => m.t === "hello")).role, "phone");
  assert.equal((await computer.next((m) => m.t === "hello")).role, "computer");

  phone.send({ v: 1, t: "command", id: "cmd-1", action: "play", uri: "spotify:track:abc" });
  const relayed = await computer.next((m) => m.t === "command" && m.id === "cmd-1");
  assert.equal(relayed.seq, 1);
  assert.equal(relayed.from, "phone");
  assert.equal(relayed.action, "play");

  phone.send({ v: 1, t: "command", id: "cmd-1", action: "play" });
  const duplicate = await phone.next((m) => m.t === "reply" && m.id === "cmd-1");
  assert.equal(duplicate.duplicate, true);
  await assert.rejects(computer.next((m) => m.t === "command" && m.id === "cmd-1", 300));

  const status = await fetch(`${ctx.base}/status`, { headers: { authorization: `Bearer ${ADMIN}` } });
  const view = await status.json();
  assert.equal(view.connections.phone, 1);
  assert.equal(view.connections.computer, 1);
});

test("replays only what a reconnecting client missed", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const phone = await openClient(liveUrl(ctx.wsBase, token("live", "phone")));
  const computer = await openClient(liveUrl(ctx.wsBase, token("live", "computer")));
  t.after(async () => {
    await phone.close();
    await computer.close();
  });
  await phone.next((m) => m.t === "hello");
  await computer.next((m) => m.t === "hello");

  phone.send({ v: 1, t: "command", id: "before", n: 1 });
  const first = await computer.next((m) => m.t === "command" && m.id === "before");
  assert.equal(first.seq, 1);
  await computer.close();

  phone.send({ v: 1, t: "command", id: "missed-1", n: 2 });
  phone.send({ v: 1, t: "command", id: "missed-2", n: 3 });
  await wait(100);

  const reconnected = await openClient(liveUrl(ctx.wsBase, token("live", "computer"), first.seq));
  t.after(() => reconnected.close());
  const hello = await reconnected.next((m) => m.t === "hello");
  assert.equal(hello.reset, false);
  assert.equal(hello.replayed, 2);
  const missedOne = await reconnected.next((m) => m.t === "command" && m.id === "missed-1");
  const missedTwo = await reconnected.next((m) => m.t === "command" && m.id === "missed-2");
  assert.deepEqual([missedOne.seq, missedTwo.seq], [2, 3]);
});

test("keeps the latest state for each role and hands it to a reconnecting client", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const phone = await openClient(liveUrl(ctx.wsBase, token("live", "phone")));
  const computer = await openClient(liveUrl(ctx.wsBase, token("live", "computer")));
  t.after(async () => {
    await phone.close();
    await computer.close();
  });
  await phone.next((m) => m.t === "hello");
  await computer.next((m) => m.t === "hello");

  computer.send({ v: 1, t: "state", id: "state-1", playing: true, position: 12 });
  const state = await phone.next((m) => m.t === "state" && m.id === "state-1");
  const stateSeq = state.seq;
  // The server's clock travels with the frame, so a replay can be aged.
  assert.equal(Number.isSafeInteger(state.at), true);
  assert.ok(state.at > 0);
  await phone.close();
  computer.send({ v: 1, t: "state", id: "state-2", playing: false, position: 40 });
  await wait(80);
  const phoneAgain = await openClient(liveUrl(ctx.wsBase, token("live", "phone"), stateSeq));
  t.after(() => phoneAgain.close());
  const hello = await phoneAgain.next((m) => m.t === "hello");
  assert.equal(hello.state.body.id, "state-2");
  assert.equal(Number.isSafeInteger(hello.state.body.at), true);
  assert.equal(hello.replayed, 1);
});

test("bad input gets a small error and never kills the server", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const phone = await openClient(liveUrl(ctx.wsBase, token("live", "phone")));
  t.after(() => phone.close());
  await phone.next((m) => m.t === "hello");

  phone.ws.send("{");
  const invalid = await phone.next((m) => m.t === "reply" && m.ok === false);
  assert.equal(invalid.error, "invalid_json");
  phone.send({ v: 1, t: "nonsense", id: "x" });
  assert.equal((await phone.next((m) => m.ok === false && m.error === "invalid_envelope")).error, "invalid_envelope");
  phone.send({ v: 1, t: "command" });
  assert.equal((await phone.next((m) => m.ok === false && m.error === "invalid_id")).error, "invalid_id");

  const health = await fetch(`${ctx.base}/health`);
  assert.equal(health.status, 200);
});

test("rejects a browser origin that is not allowed", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  await assert.rejects(
    openClient(liveUrl(ctx.wsBase, token("live", "phone")), { headers: { Origin: "https://evil.example" } }),
  );
  await assert.rejects(openClient(liveUrl(ctx.wsBase, token("media", "phone"))));
});

test("closes a connection that sends a message over the size limit", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const phone = await openClient(liveUrl(ctx.wsBase, token("live", "phone")));
  t.after(() => phone.close());
  await phone.next((m) => m.t === "hello");
  phone.ws.send("x".repeat(70 * 1024));
  const [code] = await closedEvent(phone.ws);
  assert.equal(code, 1009);
});

test("rate limits a fast sender without crashing", async (t) => {
  const ctx = await startEdge({ env: { EDGE_WS_RATE_PER_SECOND: "1", EDGE_WS_RATE_BURST: "2" } });
  t.after(() => ctx.close());
  const phone = await openClient(liveUrl(ctx.wsBase, token("live", "phone")));
  const computer = await openClient(liveUrl(ctx.wsBase, token("live", "computer")));
  t.after(async () => {
    await phone.close();
    await computer.close();
  });
  await phone.next((m) => m.t === "hello");
  await computer.next((m) => m.t === "hello");
  for (let index = 0; index < 8; index += 1) {
    phone.send({ v: 1, t: "command", id: `burst-${index}` });
  }
  const limited = await phone.next((m) => m.ok === false && m.error === "rate_limited");
  assert.equal(limited.error, "rate_limited");
  assert.equal((await fetch(`${ctx.base}/health`)).status, 200);
});

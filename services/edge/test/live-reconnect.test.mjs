// A reconnect asks for everything after its last seen sequence number. These
// tests cover the ring buffer's limit, a server restart (a new epoch), repeat
// asks and the order of what comes back.
import assert from "node:assert/strict";
import test from "node:test";
import { mintToken } from "../src/tokens.mjs";
import { SECRET, startEdge } from "./helpers/context.mjs";
import { openClient } from "./helpers/ws.mjs";

const token = (role, sub = "listener") => mintToken({ sub, role, scope: "live", ttlSeconds: 120 }, SECRET);
const liveUrl = (base, role, since) => `${base}/v1/live?token=${encodeURIComponent(token(role))}${since === undefined ? "" : `&since=${since}`}`;
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function pair(ctx) {
  const phone = await openClient(liveUrl(ctx.wsBase, "phone"));
  const computer = await openClient(liveUrl(ctx.wsBase, "computer"));
  await phone.next((m) => m.t === "hello");
  await computer.next((m) => m.t === "hello");
  return { phone, computer };
}

test("binary frames go through the same rate limit as JSON messages", {
  todo: "live.mjs #message answers binary frames before #takeToken, so a flood of them never spends a token or adds a strike",
}, async (t) => {
  const ctx = await startEdge({ env: { EDGE_WS_RATE_PER_SECOND: "1", EDGE_WS_RATE_BURST: "2" } });
  t.after(() => ctx.close());
  const { phone, computer } = await pair(ctx);
  t.after(async () => {
    await phone.close();
    await computer.close();
  });
  for (let index = 0; index < 30; index += 1) phone.ws.send(Buffer.from([1, 2, 3]));
  await wait(150);
  phone.send({ v: 1, t: "command", id: "after-binary", action: "play" });
  let relayed = null;
  try {
    relayed = await computer.next((m) => m.t === "command" && m.id === "after-binary", 400);
  } catch {
    // Nothing came through, which is the expected refusal.
  }
  assert.equal(relayed, null, "a sender past the limit must not still have commands relayed");
});

test("replay from beyond the ring buffer is contiguous and flags the gap", async (t) => {
  const ctx = await startEdge({ env: { EDGE_LIVE_BUFFER_MESSAGES: "10" } });
  t.after(() => ctx.close());
  const { phone, computer } = await pair(ctx);
  t.after(async () => {
    await phone.close();
    await computer.close();
  });
  for (let index = 1; index <= 15; index += 1) phone.send({ v: 1, t: "command", id: `c${index}` });
  await computer.next((m) => m.t === "command" && m.id === "c15");
  await computer.close();

  // Since 5 is the oldest entry minus one: still contiguous, ten messages back.
  const atEdge = await openClient(liveUrl(ctx.wsBase, "computer", 5));
  t.after(() => atEdge.close());
  const edgeHello = await atEdge.next((m) => m.t === "hello");
  assert.equal(edgeHello.reset, false);
  assert.equal(edgeHello.replayed, 10);
  const seen = [];
  for (let index = 0; index < edgeHello.replayed; index += 1) {
    const message = await atEdge.next((m) => m.t === "command" && !seen.includes(m.id), 1000);
    seen.push(message.id);
  }
  assert.deepEqual(seen, ["c6", "c7", "c8", "c9", "c10", "c11", "c12", "c13", "c14", "c15"]);

  // One earlier and the missing message cannot be replayed: reset is the answer.
  const beforeEdge = await openClient(liveUrl(ctx.wsBase, "computer", 4));
  t.after(() => beforeEdge.close());
  const gapHello = await beforeEdge.next((m) => m.t === "hello");
  assert.equal(gapHello.reset, true);
  assert.equal(gapHello.replayed, 10, "what is still held is replayed, with the reset flag");
});

test("a live message after a replay continues the sequence without a duplicate", async (t) => {
  const ctx = await startEdge({ env: { EDGE_LIVE_BUFFER_MESSAGES: "10" } });
  t.after(() => ctx.close());
  const { phone, computer } = await pair(ctx);
  t.after(async () => {
    await phone.close();
    await computer.close();
  });
  for (let index = 1; index <= 12; index += 1) phone.send({ v: 1, t: "command", id: `c${index}` });
  await computer.next((m) => m.t === "command" && m.id === "c12");
  await computer.close();

  const re = await openClient(liveUrl(ctx.wsBase, "computer", 5));
  t.after(() => re.close());
  const hello = await re.next((m) => m.t === "hello");
  const ids = [];
  const seqs = [];
  for (let index = 0; index < hello.replayed; index += 1) {
    const message = await re.next((m) => m.t === "command" && !ids.includes(m.id), 1000);
    ids.push(message.id);
    seqs.push(message.seq);
  }
  assert.deepEqual(ids, ["c6", "c7", "c8", "c9", "c10", "c11", "c12"]);

  phone.send({ v: 1, t: "command", id: "c13" });
  const next = await re.next((m) => m.t === "command" && m.id === "c13");
  ids.push(next.id);
  seqs.push(next.seq);
  assert.deepEqual(seqs, [6, 7, 8, 9, 10, 11, 12, 13], "the replay and the live tail are one order");
  assert.equal(new Set(ids).size, ids.length);
});

test("asking again from the same point replays the same messages, at least once", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const { phone, computer } = await pair(ctx);
  t.after(async () => {
    await phone.close();
    await computer.close();
  });
  for (let index = 1; index <= 4; index += 1) phone.send({ v: 1, t: "command", id: `c${index}` });
  await computer.next((m) => m.t === "command" && m.id === "c4");
  const one = await openClient(liveUrl(ctx.wsBase, "computer", 2));
  const two = await openClient(liveUrl(ctx.wsBase, "computer", 2));
  t.after(async () => {
    await one.close();
    await two.close();
  });
  const first = await one.next((m) => m.t === "hello");
  const second = await two.next((m) => m.t === "hello");
  assert.equal(first.replayed, 2);
  assert.equal(second.replayed, 2);
  assert.equal((await one.next((m) => m.t === "command" && m.id === "c3")).seq, 3);
  assert.equal((await two.next((m) => m.t === "command" && m.id === "c3")).seq, 3);
});

test("a reconnect that is already current gets no replay and no reset", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const { phone, computer } = await pair(ctx);
  t.after(async () => {
    await phone.close();
    await computer.close();
  });
  phone.send({ v: 1, t: "command", id: "one" });
  await computer.next((m) => m.t === "command" && m.id === "one");
  const re = await openClient(liveUrl(ctx.wsBase, "computer", 1));
  t.after(() => re.close());
  const hello = await re.next((m) => m.t === "hello");
  assert.equal(hello.seq, 1);
  assert.equal(hello.replayed, 0);
  assert.equal(hello.reset, false);
});

test("a restarted server has a new epoch and asks the client to reset", async (t) => {
  const first = await startEdge();
  const phone = await openClient(liveUrl(first.wsBase, "phone"));
  await phone.next((m) => m.t === "hello");
  phone.send({ v: 1, t: "command", id: "one" });
  await wait(80);
  const epoch = first.edge.live.epoch;
  await phone.close();
  await first.close();

  const second = await startEdge();
  t.after(() => second.close());
  const re = await openClient(liveUrl(second.wsBase, "phone", 1));
  t.after(() => re.close());
  const hello = await re.next((m) => m.t === "hello");
  assert.notEqual(hello.epoch, epoch);
  assert.equal(hello.reset, true, "the old sequence number is ahead of this server");
  assert.equal(hello.seq, 0);
  assert.equal(hello.replayed, 0);
});

test("an unusable since value never takes the server down", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  for (const since of ["abc", "-1", "1.5", "99999999999999999999", "%00"]) {
    const client = await openClient(`${ctx.wsBase}/v1/live?token=${encodeURIComponent(token("phone"))}&since=${since}`);
    const hello = await client.next((m) => m.t === "hello");
    assert.equal(typeof hello.seq, "number", since);
    await client.close();
  }
  assert.equal((await fetch(`${ctx.base}/health`)).status, 200);
});


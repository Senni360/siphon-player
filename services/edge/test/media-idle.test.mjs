// A client that reads four bytes and then stops must not hold a socket and a
// file descriptor forever. The media response has its own idle timeout.
import assert from "node:assert/strict";
import { once } from "node:events";
import net from "node:net";
import test from "node:test";
import { mintToken } from "../src/tokens.mjs";
import { SECRET, startEdge } from "./helpers/context.mjs";

const SUB = "slow-user";

function closedWithin(socket, ms) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("socket stayed open")), ms);
    socket.once("close", () => {
      clearTimeout(timer);
      resolve();
    });
    socket.once("error", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

test("a stalled media reader is disconnected after the idle timeout", async (t) => {
  const ctx = await startEdge({ env: { EDGE_MEDIA_IDLE_TIMEOUT_MS: "300" } });
  t.after(() => ctx.close());
  const token = mintToken({ sub: SUB, role: "computer", scope: "media", ttlSeconds: 120 }, SECRET);
  const body = Buffer.concat([Buffer.from([0, 0, 0, 0x18]), Buffer.from("ftypM4A "), Buffer.alloc(2048, 0x21)]);
  const uploaded = await fetch(`${ctx.base}/v1/upload/slow-file`, {
    method: "PUT",
    headers: { authorization: `Bearer ${token}`, "content-type": "application/octet-stream" },
    body,
  });
  assert.equal(uploaded.status, 201);
  const saved = await uploaded.json();

  const address = new URL(ctx.base);
  const socket = net.connect(Number(address.port), address.hostname);
  t.after(() => socket.destroy());
  await once(socket, "connect");
  socket.write(
    `GET ${saved.url} HTTP/1.1\r\nHost: localhost\r\nRange: bytes=0-15\r\nConnection: keep-alive\r\n\r\n`,
  );
  const [chunk] = await once(socket, "data");
  assert.match(chunk.toString("latin1"), /^HTTP\/1\.1 206/);
  socket.pause();
  await closedWithin(socket, 2_000);
});

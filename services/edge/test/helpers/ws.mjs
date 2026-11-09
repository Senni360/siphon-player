import { once } from "node:events";
import WebSocket from "ws";

/**
 * Opens a real local WebSocket client and buffers messages until someone
 * asks for them. Every test uses two of these against the same server.
 */
export async function openClient(url, { headers } = {}) {
  const ws = new WebSocket(url, headers ? { headers } : undefined);
  const queue = [];
  const waiters = [];
  ws.on("message", (data) => {
    let message;
    try {
      message = JSON.parse(data.toString("utf8"));
    } catch {
      message = { unparsable: true };
    }
    const index = waiters.findIndex((waiter) => {
      try {
        return waiter.predicate(message);
      } catch {
        return false;
      }
    });
    if (index >= 0) {
      const [waiter] = waiters.splice(index, 1);
      clearTimeout(waiter.timer);
      waiter.resolve(message);
    } else {
      queue.push(message);
    }
  });
  await once(ws, "open");
  return {
    ws,
    queue,
    next(predicate = () => true, timeout = 3000) {
      const index = queue.findIndex((message) => {
        try {
          return predicate(message);
        } catch {
          return false;
        }
      });
      if (index >= 0) return Promise.resolve(queue.splice(index, 1)[0]);
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          const at = waiters.findIndex((waiter) => waiter.timer === timer);
          if (at >= 0) waiters.splice(at, 1);
          reject(new Error("timed out waiting for a live message"));
        }, timeout);
        timer.unref?.();
        waiters.push({ predicate, resolve, timer });
      });
    },
    send(message) {
      ws.send(JSON.stringify(message));
    },
    async close() {
      if (ws.readyState === WebSocket.CLOSED) return;
      const closed = once(ws, "close").catch(() => {});
      ws.close();
      await closed;
    },
  };
}

export async function closedEvent(ws, timeout = 3000) {
  if (ws.readyState === WebSocket.CLOSED) return { code: ws.readyState };
  return Promise.race([
    once(ws, "close"),
    new Promise((_, reject) => {
      const timer = setTimeout(() => reject(new Error("socket did not close")), timeout);
      timer.unref?.();
    }),
  ]);
}

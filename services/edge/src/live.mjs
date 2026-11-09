// Realtime phone <-> computer relay.
//
// One room per user. A message from one role is relayed to the other role
// immediately, stamped with a per-user sequence number and kept for two
// minutes so a reconnect can ask for everything after its last seen number.
// Commands are deduplicated by id. Bad input produces a small error reply and
// never takes the process down.

import { randomUUID } from "node:crypto";
import { WebSocketServer, WebSocket } from "ws";
import { verifyToken } from "./tokens.mjs";

export const MESSAGE_TYPES = Object.freeze([
  "hello",
  "command",
  "state",
  "reply",
  "phone",
  "control",
  "ping",
  "pong",
]);

const TYPE_SET = new Set(MESSAGE_TYPES);
const MAX_ID = 128;
const MAX_SEEN = 2000;
const SEEN_TTL_MS = 5 * 60 * 1000;
const MAX_STRIKES = 20;

function open(ws) {
  return ws.readyState === WebSocket.OPEN;
}

function rejectUpgrade(socket, status, reason) {
  try {
    socket.write(`HTTP/1.1 ${status} ${reason}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`);
  } catch {
    // The peer may already be gone.
  }
  socket.destroy();
}

function send(ws, text) {
  if (!open(ws)) return false;
  try {
    ws.send(text);
    return true;
  } catch {
    return false;
  }
}

export class LiveHub {
  #config;
  #logger;
  #now;
  #server;
  #wss;
  #rooms = new Map();
  #connections = new Set();
  #heartbeat;
  #prune;
  #boundUpgrade;
  #closed = false;
  epoch = randomUUID();

  constructor({ server, config, logger, now = Date.now }) {
    this.#server = server;
    this.#config = config;
    this.#logger = logger;
    this.#now = now;
    this.#wss = new WebSocketServer({
      noServer: true,
      maxPayload: 64 * 1024,
      perMessageDeflate: false,
      clientTracking: false,
    });
    this.#boundUpgrade = (req, socket, head) => this.#upgrade(req, socket, head);
    server.on("upgrade", this.#boundUpgrade);
    this.#heartbeat = setInterval(() => this.#beat(), config.wsHeartbeatMs);
    this.#heartbeat.unref?.();
    this.#prune = setInterval(() => this.#pruneRooms(), 30_000);
    this.#prune.unref?.();
  }

  #room(sub) {
    let room = this.#rooms.get(sub);
    if (!room) {
      room = {
        sub,
        seq: 0,
        buffer: [],
        seen: new Map(),
        lastState: { phone: null, computer: null },
        phones: new Set(),
        computers: new Set(),
        lastAt: this.#now(),
      };
      this.#rooms.set(sub, room);
    }
    room.lastAt = this.#now();
    return room;
  }

  #roleSet(room, role) {
    return role === "phone" ? room.phones : room.computers;
  }

  #opposite(role) {
    return role === "phone" ? "computer" : "phone";
  }

  #token(req, url) {
    const header = req.headers.authorization;
    if (typeof header === "string" && header.toLowerCase().startsWith("bearer ")) return header.slice(7).trim();
    const headerToken = req.headers["x-edge-token"];
    if (typeof headerToken === "string" && headerToken.length > 0) return headerToken;
    return url.searchParams.get("token") ?? "";
  }

  #upgrade(req, socket, head) {
    if (this.#closed) {
      rejectUpgrade(socket, 503, "Service Unavailable");
      return;
    }
    let url;
    try {
      url = new URL(req.url, "http://localhost");
    } catch {
      rejectUpgrade(socket, 400, "Bad Request");
      return;
    }
    if (url.pathname !== "/v1/live") {
      rejectUpgrade(socket, 404, "Not Found");
      return;
    }
    const origin = req.headers.origin;
    if (typeof origin === "string" && !this.#config.allowedOrigins.includes(origin)) {
      rejectUpgrade(socket, 403, "Forbidden");
      return;
    }
    let claims;
    try {
      claims = verifyToken(this.#token(req, url), this.#config.tokenSecret, { scope: "live" });
    } catch {
      rejectUpgrade(socket, 401, "Unauthorized");
      return;
    }
    const sinceParam = url.searchParams.get("since");
    const since = sinceParam !== null && /^\d{1,15}$/.test(sinceParam) ? Number(sinceParam) : null;
    try {
      this.#wss.handleUpgrade(req, socket, head, (ws) => this.#accept(ws, claims, since));
    } catch {
      rejectUpgrade(socket, 500, "Internal Server Error");
    }
  }

  #accept(ws, claims, since) {
    if (this.#closed) {
      ws.close(1001, "shutting down");
      return;
    }
    const room = this.#room(claims.sub);
    const conn = {
      ws,
      sub: claims.sub,
      role: claims.role,
      room,
      alive: true,
      tokens: this.#config.wsRateBurst,
      refilledAt: this.#now(),
      strikes: 0,
    };
    this.#roleSet(room, conn.role).add(conn);
    this.#connections.add(conn);

    ws.on("pong", () => {
      conn.alive = true;
    });
    ws.on("message", (data, isBinary) => {
      try {
        this.#message(conn, data, isBinary);
      } catch {
        this.#error(conn, "", "internal");
      }
    });
    ws.on("error", () => {
      // A transport error is followed by close; nothing to log here.
    });
    ws.on("close", () => this.#drop(conn));

    this.#sendHello(conn, since, null);
  }

  #sendHello(conn, since, requestId) {
    const room = conn.room;
    let replayed = 0;
    let reset = false;
    if (since !== null) {
      if (since > room.seq) reset = true;
      const oldest = room.buffer[0]?.seq;
      if (since < room.seq && (oldest === undefined || since < oldest - 1)) reset = true;
      for (const entry of room.buffer) {
        if (entry.seq > since) {
          if (send(conn.ws, entry.text)) replayed += 1;
        }
      }
    }
    send(conn.ws, JSON.stringify({
      v: 1,
      t: "hello",
      id: requestId ?? `srv-${randomUUID()}`,
      ok: true,
      role: conn.role,
      sub: conn.sub,
      seq: room.seq,
      epoch: this.epoch,
      reset,
      replayed,
      state: room.lastState[this.#opposite(conn.role)],
      heartbeatMs: this.#config.wsHeartbeatMs,
      bufferMs: this.#config.liveBufferMs,
    }));
  }

  #message(conn, data, isBinary) {
    if (isBinary) {
      this.#error(conn, "", "binary_not_supported");
      return;
    }
    if (!this.#takeToken(conn)) return;
    let message;
    try {
      message = JSON.parse(data.toString("utf8"));
    } catch {
      this.#error(conn, "", "invalid_json");
      return;
    }
    if (!message || typeof message !== "object" || Array.isArray(message)) {
      this.#error(conn, "", "invalid_envelope");
      return;
    }
    if (message.v !== 1 || typeof message.t !== "string" || !TYPE_SET.has(message.t)) {
      this.#error(conn, typeof message.id === "string" ? message.id.slice(0, MAX_ID) : "", "invalid_envelope");
      return;
    }
    const needsId = message.t !== "ping" && message.t !== "pong";
    if (needsId && (typeof message.id !== "string" || message.id.length === 0 || message.id.length > MAX_ID)) {
      this.#error(conn, "", "invalid_id");
      return;
    }

    if (message.t === "ping") {
      send(conn.ws, JSON.stringify({ v: 1, t: "pong", id: typeof message.id === "string" ? message.id : "", seq: conn.room.seq }));
      return;
    }
    if (message.t === "pong") {
      conn.alive = true;
      return;
    }
    if (message.t === "hello") {
      const since = Number.isSafeInteger(message.since) && message.since >= 0 ? message.since : null;
      this.#sendHello(conn, since, message.id);
      return;
    }
    if (message.t === "command" && this.#seen(conn.room, message.id)) {
      send(conn.ws, JSON.stringify({ v: 1, t: "reply", id: message.id, ok: true, duplicate: true, seq: conn.room.seq }));
      return;
    }

    const room = conn.room;
    room.seq += 1;
    message.seq = room.seq;
    // The sender's role is server-stamped so replay is unambiguous even when
    // both roles send `state`.
    message.from = conn.role;
    const text = JSON.stringify(message);
    const entry = { seq: room.seq, at: this.#now(), text, role: conn.role, type: message.t };
    room.buffer.push(entry);
    if (room.buffer.length > this.#config.liveBufferMessages) room.buffer.shift();
    room.lastAt = this.#now();
    if (message.t === "state" || message.t === "phone") {
      room.lastState[conn.role] = { t: message.t, seq: message.seq, body: message };
    }
    for (const target of this.#roleSet(room, this.#opposite(conn.role))) {
      send(target.ws, text);
    }
  }

  #seen(room, id) {
    const at = this.#now();
    if (room.seen.has(id)) return true;
    room.seen.set(id, at);
    if (room.seen.size > MAX_SEEN) {
      const cutoff = at - SEEN_TTL_MS;
      for (const [key, seenAt] of room.seen) {
        if (seenAt < cutoff) room.seen.delete(key);
      }
      while (room.seen.size > MAX_SEEN) room.seen.delete(room.seen.keys().next().value);
    }
    return false;
  }

  #takeToken(conn) {
    const now = this.#now();
    const elapsed = Math.max(0, now - conn.refilledAt) / 1000;
    conn.refilledAt = now;
    conn.tokens = Math.min(this.#config.wsRateBurst, conn.tokens + elapsed * this.#config.wsRatePerSecond);
    if (conn.tokens >= 1) {
      conn.tokens -= 1;
      conn.strikes = Math.max(0, conn.strikes - 1);
      return true;
    }
    conn.strikes += 1;
    this.#error(conn, "", "rate_limited");
    if (conn.strikes >= MAX_STRIKES) {
      try {
        conn.ws.close(1008, "rate limit");
      } catch {
        // Already closed.
      }
    }
    return false;
  }

  #error(conn, id, code) {
    send(conn.ws, JSON.stringify({ v: 1, t: "reply", id: id ?? "", ok: false, error: code }));
  }

  #beat() {
    const now = this.#now();
    for (const conn of this.#connections) {
      if (!conn.alive) {
        try {
          conn.ws.terminate();
        } catch {
          // Already gone.
        }
        continue;
      }
      conn.alive = false;
      try {
        conn.ws.ping();
      } catch {
        // Close will follow.
      }
    }
    for (const room of this.#rooms.values()) {
      for (const [id, seenAt] of room.seen) {
        if (now - seenAt > SEEN_TTL_MS) room.seen.delete(id);
      }
    }
  }

  #pruneRooms() {
    const now = this.#now();
    for (const room of this.#rooms.values()) {
      const cutoff = now - this.#config.liveBufferMs;
      while (room.buffer.length > 0 && room.buffer[0].at < cutoff) room.buffer.shift();
      if (room.phones.size === 0 && room.computers.size === 0 && room.buffer.length === 0
        && now - room.lastAt > this.#config.liveBufferMs) {
        this.#rooms.delete(room.sub);
      }
    }
  }

  #drop(conn) {
    this.#connections.delete(conn);
    this.#roleSet(conn.room, conn.role).delete(conn);
    conn.room.lastAt = this.#now();
  }

  stats() {
    let phones = 0;
    let computers = 0;
    for (const conn of this.#connections) {
      if (conn.role === "phone") phones += 1;
      else computers += 1;
    }
    return { phone: phones, computer: computers, total: phones + computers, rooms: this.#rooms.size, epoch: this.epoch };
  }

  async close() {
    this.#closed = true;
    clearInterval(this.#heartbeat);
    clearInterval(this.#prune);
    this.#server.off("upgrade", this.#boundUpgrade);
    for (const conn of this.#connections) {
      try {
        conn.ws.close(1001, "shutting down");
      } catch {
        // Already closed.
      }
    }
    await new Promise((resolve) => {
      const timer = setTimeout(resolve, 1000);
      timer.unref?.();
      this.#wss.close(() => {
        clearTimeout(timer);
        resolve();
      });
    });
  }
}

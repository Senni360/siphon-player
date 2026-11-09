import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { loadConfig } from "../../src/config.mjs";
import { createLogger } from "../../src/log.mjs";
import { createEdgeServer } from "../../src/server.mjs";

export const SECRET = "local-test-secret-0123456789abcdef";
export const ADMIN = "local-test-admin-0123456789abcdef";
export const PHONE_ORIGIN = "http://localhost:5173";

export async function tempDir(prefix = "encore-edge-test-") {
  return fs.mkdtemp(path.join(os.tmpdir(), prefix));
}

export async function startEdge({ env = {}, spawn, fetch: fetchImpl } = {}) {
  const dir = await tempDir();
  const config = loadConfig({
    EDGE_TOKEN_SECRET: SECRET,
    EDGE_DATA_DIR: dir,
    EDGE_ADMIN_TOKEN: ADMIN,
    EDGE_CACHE_MAX_BYTES: env.EDGE_CACHE_MAX_BYTES ?? String(10 * 1024 * 1024),
    EDGE_UPLOAD_FILE_MAX_BYTES: env.EDGE_UPLOAD_FILE_MAX_BYTES ?? String(4 * 1024 * 1024),
    EDGE_UPLOAD_USER_MAX_BYTES: env.EDGE_UPLOAD_USER_MAX_BYTES ?? String(8 * 1024 * 1024),
    EDGE_SYNC_USER_MAX_BYTES: env.EDGE_SYNC_USER_MAX_BYTES ?? String(32 * 1024),
    EDGE_WS_RATE_PER_SECOND: env.EDGE_WS_RATE_PER_SECOND ?? "40",
    EDGE_WS_RATE_BURST: env.EDGE_WS_RATE_BURST ?? "80",
    ...env,
    EDGE_TOKEN_SECRET: SECRET,
    EDGE_DATA_DIR: dir,
  });
  const logs = [];
  const logger = createLogger({ level: "debug", sink: { log: (line) => logs.push(line) } });
  const edge = await createEdgeServer(config, { logger, spawn, fetch: fetchImpl });
  const address = await edge.listen({ port: 0, host: "127.0.0.1" });
  const base = `http://127.0.0.1:${address.port}`;
  return {
    edge,
    config,
    dir,
    base,
    wsBase: `ws://127.0.0.1:${address.port}`,
    logs,
    async close() {
      await edge.close();
      await fs.rm(dir, { recursive: true, force: true });
    },
  };
}

export async function request(base, pathname, { method = "GET", token, body, headers = {}, origin } = {}) {
  const init = { method, headers: { ...headers } };
  if (token !== undefined) init.headers.authorization = `Bearer ${token}`;
  if (origin !== undefined) init.headers.origin = origin;
  if (body !== undefined) {
    init.body = Buffer.isBuffer(body) ? body : JSON.stringify(body);
    if (!Buffer.isBuffer(body)) init.headers["content-type"] = "application/json";
  }
  return fetch(`${base}${pathname}`, init);
}

export async function jsonBody(response) {
  return response.json();
}

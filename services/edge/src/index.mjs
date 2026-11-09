// Entry point. Refuses to start without EDGE_TOKEN_SECRET. Stops cleanly on
// SIGINT/SIGTERM: no new work, live sockets closed, state flushed.

import { loadConfig, ConfigError } from "./config.mjs";
import { createLogger } from "./log.mjs";
import { createEdgeServer } from "./server.mjs";

let config;
try {
  config = loadConfig();
} catch (error) {
  if (error instanceof ConfigError) {
    process.stderr.write(`encore-edge: ${error.message}\n`);
  } else {
    process.stderr.write("encore-edge: invalid configuration\n");
  }
  process.exit(1);
}

const logger = createLogger({ level: config.logLevel });
let edge;

try {
  edge = await createEdgeServer(config, { logger });
  const address = await edge.listen();
  logger.info("started", {
    host: typeof address === "object" ? address.address : config.host,
    port: typeof address === "object" ? address.port : config.port,
    cacheCapBytes: config.cacheMaxBytes,
    resolveConcurrency: config.resolveConcurrency,
  });
} catch {
  logger.error("start_failed");
  process.exit(1);
}

let stopping = false;
async function shutdown(signal) {
  if (stopping) return;
  stopping = true;
  logger.info("stopping", { signal });
  const timer = setTimeout(() => {
    logger.error("stop_timeout");
    process.exit(1);
  }, 10_000);
  timer.unref?.();
  try {
    await edge.close();
    clearTimeout(timer);
    logger.info("stopped");
    process.exit(0);
  } catch {
    logger.error("stop_failed");
    process.exit(1);
  }
}

process.on("SIGINT", () => void shutdown("SIGINT"));
process.on("SIGTERM", () => void shutdown("SIGTERM"));
process.on("unhandledRejection", () => {
  // A rejected background job must not take the service down; its own caller
  // already received an error, and no secret is logged here.
  logger.error("unhandled_rejection");
});
process.on("uncaughtException", (error) => {
  logger.error("uncaught_exception", { code: error?.code ?? "error" });
  void shutdown("uncaughtException");
});

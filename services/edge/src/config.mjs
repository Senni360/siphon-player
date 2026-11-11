// Configuration comes from the environment only. Nothing here reads a file or
// logs a secret; a missing token secret refuses to start.

import path from "node:path";
import { MIN_SECRET_LENGTH } from "./tokens.mjs";

const GIB = 1024 ** 3;
const MIB = 1024 ** 2;

export const DEFAULTS = Object.freeze({
  port: 8787,
  host: "127.0.0.1",
  dataDir: "./data",
  cacheMaxBytes: 10 * GIB,
  uploadUserMaxBytes: 5 * GIB,
  uploadFileMaxBytes: 512 * MIB,
  syncUserMaxBytes: 4 * MIB,
  mediaTokenTtlSeconds: 60 * 60,
  wsRatePerSecond: 40,
  wsRateBurst: 80,
  wsHeartbeatMs: 15_000,
  liveBufferMs: 120_000,
  liveBufferMessages: 500,
  liveBufferBytes: 1 * MIB,
  liveEnvelopeMaxBytes: 16 * 1024,
  resolveConcurrency: 2,
  warmQueueMax: 4,
  resolveTimeoutMs: 180_000,
  ffmpegTimeoutMs: 120_000,
  mediaIdleTimeoutMs: 120_000,
  searchTimeoutMs: 45_000,
  maxJsonBytes: 8 * MIB,
  allowedOrigins: "https://encore.senni.dev,http://localhost:5173,http://127.0.0.1:5173",
  ytdlpBin: "yt-dlp",
  ffmpegBin: "ffmpeg",
  spotifyCacheTtlMs: 60_000,
  spotifyHomeCacheTtlMs: 10 * 60_000,
  spotifySearchTtlMs: 2 * 60_000,
  libraryTimeoutMs: 15_000,
  homeArtistLimit: 20,
});

const VAULT_KEY = /^[0-9a-fA-F]{64}$/;

export class ConfigError extends Error {
  constructor(message) {
    super(message);
    this.name = "ConfigError";
  }
}

function integer(value, fallback, { min = 0, max = Number.MAX_SAFE_INTEGER, name }) {
  if (value === undefined || value === "") return fallback;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < min || parsed > max) {
    throw new ConfigError(`${name} must be an integer between ${min} and ${max}`);
  }
  return parsed;
}

function bytes(value, fallback, { min = 1, max = Number.MAX_SAFE_INTEGER, name }) {
  return integer(value, fallback, { min, max, name });
}

function list(value) {
  return value
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0);
}

export function loadConfig(env = process.env) {
  const secret = env.EDGE_TOKEN_SECRET;
  if (typeof secret !== "string" || secret.length < MIN_SECRET_LENGTH) {
    throw new ConfigError(`EDGE_TOKEN_SECRET is required and must be at least ${MIN_SECRET_LENGTH} characters`);
  }
  const vaultKey = env.EDGE_VAULT_KEY;
  if (vaultKey !== undefined && vaultKey !== "" && !VAULT_KEY.test(vaultKey)) {
    throw new ConfigError("EDGE_VAULT_KEY must be 64 hexadecimal characters");
  }
  const allowedOrigins = list(env.EDGE_ALLOWED_ORIGINS ?? DEFAULTS.allowedOrigins);
  if (allowedOrigins.length === 0) {
    throw new ConfigError("EDGE_ALLOWED_ORIGINS must list at least one browser origin");
  }
  for (const origin of allowedOrigins) {
    let parsed;
    try {
      parsed = new URL(origin);
    } catch {
      throw new ConfigError("EDGE_ALLOWED_ORIGINS contains an invalid origin");
    }
    if (!["http:", "https:"].includes(parsed.protocol) || parsed.origin !== origin) {
      throw new ConfigError("EDGE_ALLOWED_ORIGINS entries must be exact http(s) origins");
    }
  }
  return Object.freeze({
    port: integer(env.EDGE_PORT ?? env.PORT, DEFAULTS.port, { max: 65535, name: "EDGE_PORT" }),
    host: env.EDGE_HOST ?? DEFAULTS.host,
    dataDir: path.resolve(env.EDGE_DATA_DIR ?? DEFAULTS.dataDir),
    tokenSecret: secret,
    adminToken: env.EDGE_ADMIN_TOKEN ?? "",
    allowedOrigins: Object.freeze(allowedOrigins),
    cacheMaxBytes: bytes(env.EDGE_CACHE_MAX_BYTES, DEFAULTS.cacheMaxBytes, { name: "EDGE_CACHE_MAX_BYTES" }),
    uploadUserMaxBytes: bytes(env.EDGE_UPLOAD_USER_MAX_BYTES, DEFAULTS.uploadUserMaxBytes, {
      name: "EDGE_UPLOAD_USER_MAX_BYTES",
    }),
    uploadFileMaxBytes: bytes(env.EDGE_UPLOAD_FILE_MAX_BYTES, DEFAULTS.uploadFileMaxBytes, {
      name: "EDGE_UPLOAD_FILE_MAX_BYTES",
    }),
    syncUserMaxBytes: bytes(env.EDGE_SYNC_USER_MAX_BYTES, DEFAULTS.syncUserMaxBytes, { name: "EDGE_SYNC_USER_MAX_BYTES" }),
    mediaTokenTtlSeconds: integer(env.EDGE_MEDIA_TOKEN_TTL_SECONDS, DEFAULTS.mediaTokenTtlSeconds, {
      min: 30,
      max: 7 * 24 * 60 * 60,
      name: "EDGE_MEDIA_TOKEN_TTL_SECONDS",
    }),
    wsRatePerSecond: integer(env.EDGE_WS_RATE_PER_SECOND, DEFAULTS.wsRatePerSecond, {
      min: 1,
      max: 1000,
      name: "EDGE_WS_RATE_PER_SECOND",
    }),
    wsRateBurst: integer(env.EDGE_WS_RATE_BURST, DEFAULTS.wsRateBurst, {
      min: 1,
      max: 10_000,
      name: "EDGE_WS_RATE_BURST",
    }),
    wsHeartbeatMs: integer(env.EDGE_WS_HEARTBEAT_MS, DEFAULTS.wsHeartbeatMs, {
      min: 1000,
      max: 120_000,
      name: "EDGE_WS_HEARTBEAT_MS",
    }),
    liveBufferMs: integer(env.EDGE_LIVE_BUFFER_MS, DEFAULTS.liveBufferMs, {
      min: 1000,
      max: 15 * 60_000,
      name: "EDGE_LIVE_BUFFER_MS",
    }),
    liveBufferMessages: integer(env.EDGE_LIVE_BUFFER_MESSAGES, DEFAULTS.liveBufferMessages, {
      min: 10,
      max: 10_000,
      name: "EDGE_LIVE_BUFFER_MESSAGES",
    }),
    liveBufferBytes: bytes(env.EDGE_LIVE_BUFFER_BYTES, DEFAULTS.liveBufferBytes, {
      min: 16 * 1024,
      max: 64 * MIB,
      name: "EDGE_LIVE_BUFFER_BYTES",
    }),
    liveEnvelopeMaxBytes: bytes(env.EDGE_LIVE_ENVELOPE_MAX_BYTES, DEFAULTS.liveEnvelopeMaxBytes, {
      min: 1024,
      max: 64 * 1024,
      name: "EDGE_LIVE_ENVELOPE_MAX_BYTES",
    }),
    // A hard ceiling: two encoders, never more, keeps a small box predictable.
    resolveConcurrency: Math.min(2, integer(env.EDGE_RESOLVE_CONCURRENCY, DEFAULTS.resolveConcurrency, {
      min: 1,
      max: 2,
      name: "EDGE_RESOLVE_CONCURRENCY",
    })),
    warmQueueMax: integer(env.EDGE_WARM_QUEUE_MAX, DEFAULTS.warmQueueMax, {
      min: 0,
      max: 32,
      name: "EDGE_WARM_QUEUE_MAX",
    }),
    resolveTimeoutMs: integer(env.EDGE_RESOLVE_TIMEOUT_MS, DEFAULTS.resolveTimeoutMs, {
      min: 1000,
      name: "EDGE_RESOLVE_TIMEOUT_MS",
    }),
    ffmpegTimeoutMs: integer(env.EDGE_FFMPEG_TIMEOUT_MS, DEFAULTS.ffmpegTimeoutMs, {
      min: 1000,
      name: "EDGE_FFMPEG_TIMEOUT_MS",
    }),
    mediaIdleTimeoutMs: integer(env.EDGE_MEDIA_IDLE_TIMEOUT_MS, DEFAULTS.mediaIdleTimeoutMs, {
      min: 250,
      max: 60 * 60_000,
      name: "EDGE_MEDIA_IDLE_TIMEOUT_MS",
    }),
    searchTimeoutMs: integer(env.EDGE_SEARCH_TIMEOUT_MS, DEFAULTS.searchTimeoutMs, {
      min: 1000,
      name: "EDGE_SEARCH_TIMEOUT_MS",
    }),
    maxJsonBytes: bytes(env.EDGE_MAX_JSON_BYTES, DEFAULTS.maxJsonBytes, { name: "EDGE_MAX_JSON_BYTES" }),
    ytdlpBin: env.EDGE_YTDLP_BIN || DEFAULTS.ytdlpBin,
    ffmpegBin: env.EDGE_FFMPEG_BIN || DEFAULTS.ffmpegBin,
    vaultKey: vaultKey === undefined || vaultKey === "" ? null : vaultKey.toLowerCase(),
    spotifyCacheTtlMs: integer(env.EDGE_SPOTIFY_CACHE_TTL_MS, DEFAULTS.spotifyCacheTtlMs, {
      min: 0,
      max: 60 * 60_000,
      name: "EDGE_SPOTIFY_CACHE_TTL_MS",
    }),
    spotifyHomeCacheTtlMs: integer(env.EDGE_SPOTIFY_HOME_CACHE_TTL_MS, DEFAULTS.spotifyHomeCacheTtlMs, {
      min: 0,
      max: 24 * 60 * 60_000,
      name: "EDGE_SPOTIFY_HOME_CACHE_TTL_MS",
    }),
    spotifySearchTtlMs: integer(env.EDGE_SPOTIFY_SEARCH_TTL_MS, DEFAULTS.spotifySearchTtlMs, {
      min: 0,
      max: 60 * 60_000,
      name: "EDGE_SPOTIFY_SEARCH_TTL_MS",
    }),
    libraryTimeoutMs: integer(env.EDGE_LIBRARY_TIMEOUT_MS, DEFAULTS.libraryTimeoutMs, {
      min: 1000,
      max: 120_000,
      name: "EDGE_LIBRARY_TIMEOUT_MS",
    }),
    homeArtistLimit: integer(env.EDGE_HOME_ARTIST_LIMIT, DEFAULTS.homeArtistLimit, {
      min: 1,
      max: 50,
      name: "EDGE_HOME_ARTIST_LIMIT",
    }),
    logLevel: env.EDGE_LOG_LEVEL ?? "info",
  });
}

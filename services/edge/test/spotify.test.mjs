import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { createLogger } from "../src/log.mjs";
import { SpotifyClient } from "../src/spotify.mjs";
import { Vault } from "../src/vault.mjs";
import { tempDir } from "./helpers/context.mjs";
import { fakeFetch, header, isSpotifyApi, isSpotifyToken, reply } from "./helpers/http.mjs";

const KEY = "33".repeat(32);
const SUB = "spotify-user";
const CLIENT_ID = "b".repeat(32);
const OLD_REFRESH = "old-refresh-token-0001";
const ROTATED_REFRESH = "rotated-refresh-token-0002";

async function vaultWithSpotify() {
  const dir = await tempDir();
  const vault = new Vault({ dir, key: KEY });
  await vault.putSpotify(SUB, { clientId: CLIENT_ID, refreshToken: OLD_REFRESH });
  return vault;
}

function captureLogger() {
  const lines = [];
  const logger = createLogger({ level: "debug", sink: { log: (line) => lines.push(line) } });
  return { logger, lines };
}

test("refreshes with the listener's own Client ID and saves a rotated token", async () => {
  const vault = await vaultWithSpotify();
  const { logger, lines } = captureLogger();
  let tokenCalls = 0;
  const fetchImpl = fakeFetch((call) => {
    if (isSpotifyToken(call)) {
      tokenCalls += 1;
      return reply(200, {
        access_token: "access-token-memory-only",
        expires_in: 3600,
        refresh_token: ROTATED_REFRESH,
      });
    }
    return reply(200, { items: [], next: null });
  });
  const client = new SpotifyClient({ vault, fetch: fetchImpl, now: () => 1_000_000, logger });

  assert.equal(await client.accessToken(SUB), "access-token-memory-only");
  assert.equal(tokenCalls, 1);

  const call = fetchImpl.calls.find(isSpotifyToken);
  const body = new URLSearchParams(call.body);
  assert.equal(body.get("grant_type"), "refresh_token");
  assert.equal(body.get("client_id"), CLIENT_ID);
  assert.equal(body.get("refresh_token"), OLD_REFRESH);
  assert.equal(header(call, "authorization"), undefined);

  const stored = await vault.getSpotify(SUB);
  assert.equal(stored.refreshToken, ROTATED_REFRESH);
  assert.equal(stored.clientId, CLIENT_ID);

  const joined = lines.join("\n");
  assert.equal(joined.includes("access-token-memory-only"), false);
  assert.equal(joined.includes(ROTATED_REFRESH), false);
  assert.equal(joined.includes(OLD_REFRESH), false);
});

test("access tokens stay in memory and never reach the vault file", async () => {
  const dir = await tempDir();
  const vault = new Vault({ dir, key: KEY });
  await vault.putSpotify(SUB, { clientId: CLIENT_ID, refreshToken: OLD_REFRESH });
  const client = new SpotifyClient({
    vault,
    fetch: fakeFetch(() => reply(200, { access_token: "memory-only-access", expires_in: 3600 })),
    now: () => 1_000_000,
  });
  assert.equal(await client.accessToken(SUB), "memory-only-access");

  const files = await fs.readdir(path.join(dir, "vault"));
  for (const file of files) {
    const text = await fs.readFile(path.join(dir, "vault", file), "utf8");
    assert.equal(text.includes("memory-only-access"), false);
  }
});

test("concurrent callers share one in-flight refresh", async () => {
  let tokenCalls = 0;
  let releaseVault;
  const vaultGate = new Promise((resolve) => {
    releaseVault = resolve;
  });
  // Holding the vault read open guarantees both callers are waiting on the
  // same refresh rather than relying on timing.
  const vault = {
    getSpotify: () => vaultGate,
    saveSpotifyRefresh: async () => {},
  };
  const fetchImpl = fakeFetch((call) => {
    if (isSpotifyToken(call)) {
      tokenCalls += 1;
      return reply(200, { access_token: "shared-access", expires_in: 3600 });
    }
    return reply(200, {});
  });
  const client = new SpotifyClient({ vault, fetch: fetchImpl, now: () => 1_000_000 });

  const first = client.accessToken(SUB);
  const second = client.accessToken(SUB);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(tokenCalls, 0);
  releaseVault({ clientId: CLIENT_ID, refreshToken: OLD_REFRESH });
  assert.equal(await first, "shared-access");
  assert.equal(await second, "shared-access");
  assert.equal(tokenCalls, 1);
});

test("a 429 pauses that listener until Retry-After has passed", async () => {
  const vault = await vaultWithSpotify();
  let clock = 2_000_000;
  let apiCalls = 0;
  const fetchImpl = fakeFetch((call) => {
    if (isSpotifyToken(call)) return reply(200, { access_token: "access", expires_in: 3600 });
    if (isSpotifyApi(call, "/v1/me/playlists")) {
      apiCalls += 1;
      if (apiCalls === 1) return reply(429, {}, { "retry-after": "2" });
      return reply(200, { items: [], next: null });
    }
    return reply(404, {});
  });
  const client = new SpotifyClient({ vault, fetch: fetchImpl, now: () => clock });

  await assert.rejects(
    () => client.api(SUB, "/me/playlists"),
    (error) => error.code === "spotify_rate_limited" && error.retryAfter === 2,
  );
  await assert.rejects(
    () => client.api(SUB, "/me/playlists"),
    (error) => error.code === "spotify_rate_limited" && error.retryAfter > 0,
  );
  assert.equal(apiCalls, 1);

  clock += 2001;
  assert.deepEqual(await client.api(SUB, "/me/playlists"), { items: [], next: null });
  assert.equal(apiCalls, 2);
});

test("a successful read is cached per listener until its TTL ends", async () => {
  const vault = await vaultWithSpotify();
  let clock = 3_000_000;
  let tokenCalls = 0;
  let apiCalls = 0;
  const fetchImpl = fakeFetch((call) => {
    if (isSpotifyToken(call)) {
      tokenCalls += 1;
      return reply(200, { access_token: "access", expires_in: 3600 });
    }
    apiCalls += 1;
    return reply(200, { items: [{ id: "a" }], next: null });
  });
  const client = new SpotifyClient({ vault, fetch: fetchImpl, now: () => clock, cacheTtlMs: 60_000 });

  await client.api(SUB, "/v1/me/albums");
  await client.api(SUB, "/v1/me/albums");
  assert.equal(tokenCalls, 1);
  assert.equal(apiCalls, 1);

  await client.api(SUB, "/v1/me/albums", { query: { limit: 20 } });
  assert.equal(apiCalls, 2);

  clock += 60_001;
  await client.api(SUB, "/v1/me/albums");
  assert.equal(apiCalls, 3);
});

test("a refused access token is replaced once, and a rejected refresh asks for a new sign-in", async () => {
  const vault = await vaultWithSpotify();
  let tokenCalls = 0;
  let apiCalls = 0;
  const fetchImpl = fakeFetch((call) => {
    if (isSpotifyToken(call)) {
      tokenCalls += 1;
      return reply(200, { access_token: `access-${tokenCalls}`, expires_in: 3600 });
    }
    apiCalls += 1;
    if (apiCalls === 1) return reply(401, {});
    return reply(200, { items: [] });
  });
  const client = new SpotifyClient({ vault, fetch: fetchImpl, now: () => 4_000_000 });
  assert.deepEqual(await client.api(SUB, "/v1/me/tracks"), { items: [] });
  assert.equal(tokenCalls, 2);
  assert.equal(apiCalls, 2);

  const rejecting = new SpotifyClient({
    vault,
    fetch: fakeFetch(() => reply(400, { error: "invalid_grant" })),
    now: () => 4_000_000,
  });
  await assert.rejects(() => rejecting.accessToken(SUB), (error) => error.code === "spotify_sign_in_expired");
});

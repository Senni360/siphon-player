import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { userHash } from "../src/atomic.mjs";
import { mintToken } from "../src/tokens.mjs";
import { Vault } from "../src/vault.mjs";
import { SECRET, request, startEdge, tempDir } from "./helpers/context.mjs";

const KEY = "11".repeat(32);
const OTHER_KEY = "22".repeat(32);
const SUB = "vault-user";
const CLIENT_ID = "a".repeat(32);
const REFRESH = "refresh-token-value-123456";

function computerToken(sub = SUB) {
  return mintToken({ sub, role: "computer", scope: "sync", ttlSeconds: 120 }, SECRET);
}

test("encrypts a Spotify sign-in at rest and round-trips it", async () => {
  const dir = await tempDir();
  const vault = new Vault({ dir, key: KEY });
  await vault.putSpotify(SUB, { clientId: CLIENT_ID, refreshToken: REFRESH });

  const loaded = await vault.getSpotify(SUB);
  assert.deepEqual(loaded, { clientId: CLIENT_ID, refreshToken: REFRESH });

  const file = path.join(dir, "vault", `${userHash(SUB).slice(0, 32)}.json`);
  const raw = JSON.parse(await fs.readFile(file, "utf8"));
  assert.equal(raw.v, 1);
  assert.equal(typeof raw.iv, "string");
  assert.equal(typeof raw.tag, "string");
  assert.equal(JSON.stringify(raw).includes(REFRESH), false);
  assert.equal(JSON.stringify(raw).includes(CLIENT_ID), false);
  assert.equal(JSON.stringify(raw).includes("refresh_token"), false);
});

test("a different key cannot open the vault", async () => {
  const dir = await tempDir();
  const vault = new Vault({ dir, key: KEY });
  await vault.putSpotify(SUB, { clientId: CLIENT_ID, refreshToken: REFRESH });
  const wrong = new Vault({ dir, key: OTHER_KEY });
  await assert.rejects(() => wrong.getSpotify(SUB), (error) => error.code === "vault_key_mismatch");
});

test("deleting a provider removes it from disk, and the last one removes the file", async () => {
  const dir = await tempDir();
  const vault = new Vault({ dir, key: KEY });
  const file = path.join(dir, "vault", `${userHash(SUB).slice(0, 32)}.json`);

  await vault.putSpotify(SUB, { clientId: CLIENT_ID, refreshToken: REFRESH });
  await vault.putSoundcloud(SUB, { oauthToken: "2-000000-0000000000-soundcloud" });
  assert.equal((await vault.getSpotify(SUB)).refreshToken, REFRESH);
  assert.deepEqual(await vault.getSoundcloud(SUB), { oauthToken: "2-000000-0000000000-soundcloud" });

  await vault.deleteSpotify(SUB);
  assert.equal(await vault.getSpotify(SUB), null);
  assert.deepEqual(await vault.getSoundcloud(SUB), { oauthToken: "2-000000-0000000000-soundcloud" });
  await fs.access(file);

  await vault.deleteSoundcloud(SUB);
  assert.equal(await vault.getSoundcloud(SUB), null);
  await assert.rejects(() => fs.access(file), (error) => error.code === "ENOENT");
});

test("only a computer sync token may write the vault, and no token is echoed or logged", async (t) => {
  const ctx = await startEdge({ env: { EDGE_VAULT_KEY: KEY } });
  t.after(() => ctx.close());

  const phone = mintToken({ sub: SUB, role: "phone", scope: "sync", ttlSeconds: 120 }, SECRET);
  const computerMedia = mintToken({ sub: SUB, role: "computer", scope: "media", ttlSeconds: 120 }, SECRET);

  const phoneWrite = await request(ctx.base, "/v1/vault/spotify", {
    method: "PUT",
    token: phone,
    body: { clientId: CLIENT_ID, refreshToken: REFRESH },
  });
  assert.equal(phoneWrite.status, 401);

  const mediaWrite = await request(ctx.base, "/v1/vault/spotify", {
    method: "PUT",
    token: computerMedia,
    body: { clientId: CLIENT_ID, refreshToken: REFRESH },
  });
  assert.equal(mediaWrite.status, 401);

  const saved = await request(ctx.base, "/v1/vault/spotify", {
    method: "PUT",
    token: computerToken(),
    body: { clientId: CLIENT_ID, refreshToken: REFRESH },
  });
  assert.equal(saved.status, 200);
  const savedText = await saved.text();
  assert.equal(savedText.includes(REFRESH), false);
  assert.equal(savedText.includes(CLIENT_ID), false);

  const stored = ctx.edge.vault;
  assert.equal((await stored.getSpotify(SUB)).refreshToken, REFRESH);

  const removed = await request(ctx.base, "/v1/vault/spotify", {
    method: "DELETE",
    token: computerToken(),
  });
  assert.equal(removed.status, 200);
  assert.equal((await removed.text()).includes(REFRESH), false);
  assert.equal(await stored.getSpotify(SUB), null);

  const joined = ctx.logs.join("\n");
  for (const secret of [REFRESH, CLIENT_ID, phone, computerMedia]) {
    assert.equal(joined.includes(secret), false);
  }
});

test("vault-backed routes answer 503 when EDGE_VAULT_KEY is unset, and the service still starts", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  assert.equal((await fetch(`${ctx.base}/health`)).status, 200);

  const write = await request(ctx.base, "/v1/vault/spotify", {
    method: "PUT",
    token: computerToken(),
    body: { clientId: CLIENT_ID, refreshToken: REFRESH },
  });
  assert.equal(write.status, 503);
  assert.equal((await write.json()).error, "vault_not_configured");

  const phone = mintToken({ sub: SUB, role: "phone", scope: "sync", ttlSeconds: 120 }, SECRET);
  const home = await request(ctx.base, "/v1/home", { token: phone });
  assert.equal(home.status, 503);
  assert.equal((await home.json()).error, "vault_not_configured");
});

test("a malformed vault key refuses to start and bad credentials are refused", async () => {
  const { loadConfig } = await import("../src/config.mjs");
  assert.throws(() => loadConfig({ EDGE_TOKEN_SECRET: SECRET, EDGE_VAULT_KEY: "nope" }), /EDGE_VAULT_KEY/);

  const dir = await tempDir();
  const vault = new Vault({ dir, key: KEY });
  await assert.rejects(
    () => vault.putSpotify(SUB, { clientId: "short", refreshToken: REFRESH }),
    (error) => error.code === "invalid_spotify_client_id",
  );
  await assert.rejects(
    () => vault.putSoundcloud(SUB, { oauthToken: "oauth_token=leaked" }),
    (error) => error.code === "invalid_soundcloud_token",
  );
});

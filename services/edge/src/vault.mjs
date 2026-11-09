// The sign-in vault. Each listener's sign-ins are encrypted at rest with
// AES-256-GCM and a key that only lives in EDGE_VAULT_KEY. No route ever
// returns a token and this module never logs one.
//
// On disk: $EDGE_DATA_DIR/vault/<user-hash>.json
//   { "v": 1, "iv": "<base64>", "tag": "<base64>", "data": "<base64>" }
//
// The plaintext is small JSON holding the Spotify refresh token with the
// listener's own Client ID, and optionally a SoundCloud OAuth token.

import { createCipheriv, createDecipheriv, randomBytes } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { atomicWriteJson, KeyedQueue, userHash } from "./atomic.mjs";

const AAD_PREFIX = "encore-edge-vault\0";
const IV_BYTES = 12;
const KEY_BYTES = 32;
const FORMAT_VERSION = 1;
const SPOTIFY_CLIENT_ID = /^[A-Za-z0-9]{16,64}$/;
const PRINTABLE = /^[\u0021-\u007e]+$/;

export class VaultError extends Error {
  constructor(code, status = 400) {
    super(code);
    this.name = "VaultError";
    this.code = code;
    this.status = status;
  }
}

function keyBytes(key) {
  if (key === null || key === undefined) return null;
  if (Buffer.isBuffer(key) && key.length === KEY_BYTES) return key;
  if (typeof key === "string" && /^[0-9a-fA-F]{64}$/.test(key)) return Buffer.from(key, "hex");
  throw new VaultError("bad_vault_key", 500);
}

/** Accepts strictly a token value, never a copied cookie or header dump. */
export function normalizeSoundcloudToken(value) {
  if (typeof value !== "string") return null;
  let token = value.trim();
  if (/^oauth\s+/i.test(token)) token = token.slice(6).trim();
  if (token.length < 8 || token.length > 2048) return null;
  if (!PRINTABLE.test(token)) return null;
  if (/[;\r\n]/.test(token)) return null;
  if (/^(oauth_token|authorization|bearer)\s*[=:]/i.test(token)) return null;
  if (/^bearer\s/i.test(token)) return null;
  if (/^your_(session|access)_token$/i.test(token)) return null;
  return token;
}

function normalizeSpotify({ clientId, client_id: clientIdSnake, refreshToken, refresh_token: refreshTokenSnake }) {
  const id = clientId ?? clientIdSnake;
  const token = refreshToken ?? refreshTokenSnake;
  if (typeof id !== "string" || !SPOTIFY_CLIENT_ID.test(id)) {
    throw new VaultError("invalid_spotify_client_id", 400);
  }
  if (typeof token !== "string" || token.length < 8 || token.length > 4096 || !PRINTABLE.test(token.trim())) {
    throw new VaultError("invalid_spotify_refresh_token", 400);
  }
  const trimmed = token.trim();
  if (trimmed.length < 8 || /[\s;]/.test(trimmed)) {
    throw new VaultError("invalid_spotify_refresh_token", 400);
  }
  return { clientId: id, refreshToken: trimmed };
}

export class Vault {
  #dir;
  #key;
  #now;
  #queue = new KeyedQueue();

  constructor({ dir, key = null, now = Date.now }) {
    this.#dir = path.join(dir, "vault");
    this.#key = keyBytes(key);
    this.#now = now;
  }

  get enabled() {
    return this.#key !== null;
  }

  #assertEnabled() {
    if (!this.#key) throw new VaultError("vault_not_configured", 503);
  }

  #file(sub) {
    return path.join(this.#dir, `${userHash(sub).slice(0, 32)}.json`);
  }

  #aad(sub) {
    return Buffer.from(`${AAD_PREFIX}${userHash(sub).slice(0, 32)}`, "utf8");
  }

  #seal(sub, record) {
    const iv = randomBytes(IV_BYTES);
    const cipher = createCipheriv("aes-256-gcm", this.#key, iv);
    cipher.setAAD(this.#aad(sub));
    const data = Buffer.concat([cipher.update(Buffer.from(JSON.stringify(record), "utf8")), cipher.final()]);
    return {
      v: FORMAT_VERSION,
      iv: iv.toString("base64"),
      tag: cipher.getAuthTag().toString("base64"),
      data: data.toString("base64"),
    };
  }

  #open(sub, envelope) {
    if (!envelope || typeof envelope !== "object" || envelope.v !== FORMAT_VERSION) {
      throw new VaultError("vault_corrupt", 500);
    }
    let iv;
    let tag;
    let data;
    try {
      iv = Buffer.from(String(envelope.iv), "base64");
      tag = Buffer.from(String(envelope.tag), "base64");
      data = Buffer.from(String(envelope.data), "base64");
    } catch {
      throw new VaultError("vault_corrupt", 500);
    }
    if (iv.length !== IV_BYTES || tag.length !== 16) throw new VaultError("vault_corrupt", 500);
    try {
      const decipher = createDecipheriv("aes-256-gcm", this.#key, iv);
      decipher.setAAD(this.#aad(sub));
      decipher.setAuthTag(tag);
      const plain = Buffer.concat([decipher.update(data), decipher.final()]).toString("utf8");
      const record = JSON.parse(plain);
      if (!record || typeof record !== "object" || Array.isArray(record)) throw new Error("bad");
      return record;
    } catch {
      // A valid file that this key cannot open means the key was replaced.
      throw new VaultError("vault_key_mismatch", 500);
    }
  }

  async #read(sub) {
    this.#assertEnabled();
    let text;
    try {
      text = await fs.readFile(this.#file(sub), "utf8");
    } catch (error) {
      if (error?.code === "ENOENT") return null;
      throw error;
    }
    let envelope;
    try {
      envelope = JSON.parse(text);
    } catch {
      throw new VaultError("vault_corrupt", 500);
    }
    return this.#open(sub, envelope);
  }

  async #write(sub, record) {
    const providers = ["spotify", "soundcloud"].filter((name) => record[name]);
    if (providers.length === 0) {
      await fs.unlink(this.#file(sub)).catch((error) => {
        if (error?.code !== "ENOENT") throw error;
      });
      return;
    }
    const plain = {
      v: FORMAT_VERSION,
      updatedAt: new Date(this.#now()).toISOString(),
    };
    if (record.spotify) plain.spotify = record.spotify;
    if (record.soundcloud) plain.soundcloud = record.soundcloud;
    await atomicWriteJson(this.#file(sub), this.#seal(sub, plain));
  }

  async #mutate(sub, change) {
    this.#assertEnabled();
    const file = this.#file(sub);
    return this.#queue.run(file, async () => {
      const current = (await this.#read(sub)) ?? {};
      const next = { spotify: current.spotify, soundcloud: current.soundcloud };
      change(next);
      await this.#write(sub, next);
      return next;
    });
  }

  async putSpotify(sub, credentials) {
    const normalized = normalizeSpotify(credentials ?? {});
    return this.#mutate(sub, (record) => {
      record.spotify = normalized;
    });
  }

  /** Saves a rotated refresh token together with the Client ID it belongs to. */
  async saveSpotifyRefresh(sub, { clientId, refreshToken }) {
    return this.putSpotify(sub, { clientId, refreshToken });
  }

  async deleteSpotify(sub) {
    return this.#mutate(sub, (record) => {
      delete record.spotify;
    });
  }

  async getSpotify(sub) {
    const record = await this.#read(sub);
    if (!record?.spotify) return null;
    const { clientId, refreshToken } = record.spotify;
    if (typeof clientId !== "string" || typeof refreshToken !== "string") return null;
    return { clientId, refreshToken };
  }

  async putSoundcloud(sub, { oauthToken, oauth_token: oauthTokenSnake } = {}) {
    const token = normalizeSoundcloudToken(oauthToken ?? oauthTokenSnake);
    if (!token) throw new VaultError("invalid_soundcloud_token", 400);
    return this.#mutate(sub, (record) => {
      record.soundcloud = { oauthToken: token };
    });
  }

  async deleteSoundcloud(sub) {
    return this.#mutate(sub, (record) => {
      delete record.soundcloud;
    });
  }

  async getSoundcloud(sub) {
    const record = await this.#read(sub);
    if (!record?.soundcloud?.oauthToken) return null;
    if (typeof record.soundcloud.oauthToken !== "string") return null;
    return { oauthToken: record.soundcloud.oauthToken };
  }
}

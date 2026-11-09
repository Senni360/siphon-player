// Per-user sync documents: playlists, queue/position and the match map.
// Files are small, JSON and atomically replaced, so a restart loses nothing
// that was acknowledged. Size is capped per user across all three documents.

import fs from "node:fs/promises";
import path from "node:path";
import { atomicWriteJson, byteLength, KeyedQueue, readJsonFile, userHash } from "./atomic.mjs";

export const SYNC_DOCS = Object.freeze(["playlists", "queue", "matches"]);
const DOC_SET = new Set(SYNC_DOCS);
const SPOTIFY_URI = /^spotify:[a-z]{2,16}:[A-Za-z0-9]{1,64}$/;
const SOURCES = new Set(["youtube", "soundcloud", "local", "blob", "spotify"]);
const MAX_SOURCE_URI = 2048;

export class SyncError extends Error {
  constructor(code, status = 400) {
    super(code);
    this.name = "SyncError";
    this.code = code;
    this.status = status;
  }
}

function hasOwn(value, key) {
  return Object.prototype.hasOwnProperty.call(value, key);
}

function normalizeDate(value) {
  if (typeof value === "number" && Number.isFinite(value)) {
    const date = new Date(value);
    if (!Number.isNaN(date.getTime())) return date.toISOString();
  }
  if (typeof value === "string" && value.length <= 40) {
    const date = new Date(value);
    if (!Number.isNaN(date.getTime())) return date.toISOString();
  }
  throw new SyncError("invalid_match_date");
}

function normalizeMatch(entry) {
  if (!entry || typeof entry !== "object" || Array.isArray(entry)) throw new SyncError("invalid_match");
  if (!SOURCES.has(entry.source)) throw new SyncError("invalid_match_source");
  if (typeof entry.sourceUri !== "string" || entry.sourceUri.length === 0 || entry.sourceUri.length > MAX_SOURCE_URI
    || /[\u0000-\u001f\u007f]/.test(entry.sourceUri)) {
    throw new SyncError("invalid_match_uri");
  }
  const confidence = entry.confidence;
  if (typeof confidence !== "number" || !Number.isFinite(confidence) || confidence < 0 || confidence > 1) {
    throw new SyncError("invalid_match_confidence");
  }
  if (typeof entry.manual !== "boolean") throw new SyncError("invalid_match_manual");
  return Object.freeze({
    source: entry.source,
    sourceUri: entry.sourceUri,
    confidence,
    date: normalizeDate(entry.date),
    manual: entry.manual,
  });
}

function validateMatches(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new SyncError("invalid_matches");
  const entries = Object.entries(value);
  if (entries.length > 5000) throw new SyncError("invalid_matches");
  const normalized = {};
  for (const [uri, entry] of entries) {
    if (!SPOTIFY_URI.test(uri)) throw new SyncError("invalid_spotify_uri");
    normalized[uri] = normalizeMatch(entry);
  }
  return normalized;
}

function validateDocument(doc, data) {
  if (doc === "matches") return validateMatches(data);
  if (!data || typeof data !== "object") throw new SyncError("invalid_data");
  return data;
}

function emptyData(doc) {
  return doc === "matches" ? {} : null;
}

/**
 * A hand-chosen match always outranks an automatic one. Automatic updates can
 * refresh an automatic entry, and a hand choice can replace anything.
 */
export function mergeMatches(stored, incoming, remove = []) {
  const merged = { ...stored };
  for (const [uri, entry] of Object.entries(incoming)) {
    const current = merged[uri];
    if (current?.manual && !entry.manual) continue;
    if (current && current.manual === entry.manual && current.date > entry.date) continue;
    merged[uri] = entry;
  }
  const kept = [];
  for (const uri of remove) {
    if (!SPOTIFY_URI.test(uri)) throw new SyncError("invalid_spotify_uri");
    const current = merged[uri];
    if (!current) continue;
    if (current.manual) {
      kept.push(uri);
      continue;
    }
    delete merged[uri];
  }
  return { matches: merged, kept };
}

export class SyncStore {
  #dir;
  #limit;
  #now;
  #queue = new KeyedQueue();
  #users = new Map();

  constructor({ dir, userMaxBytes = 4 * 1024 * 1024, now = Date.now }) {
    this.#dir = path.join(dir, "sync");
    this.#limit = userMaxBytes;
    this.#now = now;
  }

  userDir(sub) {
    return path.join(this.#dir, userHash(sub).slice(0, 32));
  }

  #userSlot(sub) {
    // The directory name is a hash, never a raw account identifier.
    const key = userHash(sub).slice(0, 32);
    let slot = this.#users.get(key);
    if (!slot) {
      slot = { key, loaded: null, docs: new Map(), lastUsed: this.#now() };
      this.#users.set(key, slot);
    }
    slot.lastUsed = this.#now();
    return slot;
  }

  async #load(slot) {
    if (slot.loaded) return slot.loaded;
    slot.loaded = (async () => {
      const dir = path.join(this.#dir, slot.key);
      for (const doc of SYNC_DOCS) {
        const stored = await readJsonFile(path.join(dir, `${doc}.json`), null);
        if (stored && typeof stored === "object" && stored.doc === doc && Number.isSafeInteger(stored.revision)) {
          slot.docs.set(doc, stored);
        }
      }
      return slot;
    })();
    return slot.loaded;
  }

  #record(slot, doc, data, revision) {
    return {
      v: 1,
      doc,
      revision,
      updatedAt: new Date(this.#now()).toISOString(),
      data,
    };
  }

  async get(sub, doc) {
    if (!DOC_SET.has(doc)) throw new SyncError("unknown_document", 404);
    const slot = this.#userSlot(sub);
    await this.#load(slot);
    const record = slot.docs.get(doc);
    if (!record) return this.#record(slot, doc, emptyData(doc), 0);
    return record;
  }

  #totalBytes(slot, replacements = new Map()) {
    let total = 0;
    for (const doc of SYNC_DOCS) {
      const record = replacements.get(doc) ?? slot.docs.get(doc);
      total += record ? byteLength(record) : 0;
    }
    return total;
  }

  async put(sub, doc, body) {
    if (!DOC_SET.has(doc)) throw new SyncError("unknown_document", 404);
    if (!body || typeof body !== "object" || Array.isArray(body)) throw new SyncError("invalid_body");
    if (!hasOwn(body, "data")) throw new SyncError("invalid_body");
    const slot = this.#userSlot(sub);
    await this.#load(slot);
    return this.#queue.run(slot.key, async () => {
      const incoming = validateDocument(doc, body.data);
      const revision = body.revision;
      if (revision !== undefined && (!Number.isSafeInteger(revision) || revision < 0)) {
        throw new SyncError("invalid_revision");
      }
      const current = slot.docs.get(doc);
      if (revision !== undefined && revision !== (current?.revision ?? 0)) {
        throw new SyncError("revision_conflict", 409);
      }
      let data = incoming;
      let kept;
      if (doc === "matches") {
        const remove = body.remove ?? [];
        if (!Array.isArray(remove) || remove.length > 1000) throw new SyncError("invalid_remove");
        const merged = mergeMatches(current?.data ?? {}, incoming, remove);
        data = merged.matches;
        kept = merged.kept;
      }
      const record = this.#record(slot, doc, data, (current?.revision ?? 0) + 1);
      const total = this.#totalBytes(slot, new Map([[doc, record]]));
      if (total > this.#limit) throw new SyncError("user_limit_reached", 413);
      const dir = path.join(this.#dir, slot.key);
      await atomicWriteJson(path.join(dir, `${doc}.json`), record);
      slot.docs.set(doc, record);
      return kept && kept.length > 0 ? { ...record, kept } : record;
    });
  }

  async stats() {
    let entries = 0;
    try {
      const dirs = await fs.readdir(this.#dir, { withFileTypes: true });
      entries = dirs.filter((entry) => entry.isDirectory()).length;
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
    return { users: entries };
  }

  close() {}
}

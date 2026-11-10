// Uploaded local files live outside the disposable cache and are never
// evicted. A PUT is streamed to a temp file, checked for real audio bytes and
// a size cap, fsynced, then renamed into place.

import fs from "node:fs/promises";
import path from "node:path";
import { KeyedQueue, ownerPrefix } from "./atomic.mjs";

export const UPLOAD_KEY = /^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$/;
const OWNER_DIR = /^[0-9a-f]{16}$/;

export class UploadError extends Error {
  constructor(code, status = 400) {
    super(code);
    this.name = "UploadError";
    this.code = code;
    this.status = status;
  }
}

function audioFormat(head) {
  if (head.length >= 12 && head.toString("latin1", 4, 8) === "ftyp") {
    return { contentType: "audio/mp4", ext: "m4a" };
  }
  if (head.length >= 3 && head.toString("latin1", 0, 3) === "ID3") {
    return { contentType: "audio/mpeg", ext: "mp3" };
  }
  if (head.length >= 2 && head[0] === 0xff && (head[1] & 0xe0) === 0xe0) {
    return { contentType: "audio/mpeg", ext: "mp3" };
  }
  if (head.length >= 4 && head.toString("latin1", 0, 4) === "OggS") {
    return { contentType: "audio/ogg", ext: "ogg" };
  }
  if (head.length >= 4 && head.toString("latin1", 0, 4) === "fLaC") {
    return { contentType: "audio/flac", ext: "flac" };
  }
  if (head.length >= 12 && head.toString("latin1", 0, 4) === "RIFF" && head.toString("latin1", 8, 12) === "WAVE") {
    return { contentType: "audio/wav", ext: "wav" };
  }
  if (head.length >= 12 && head.toString("latin1", 0, 4) === "FORM") {
    const kind = head.toString("latin1", 8, 12);
    if (kind === "AIFF" || kind === "AIFC") return { contentType: "audio/aiff", ext: "aiff" };
  }
  return null;
}

export function looksLikeAudio(head) {
  return audioFormat(head) !== null;
}

function contentTypeForExt(ext) {
  return {
    m4a: "audio/mp4",
    mp3: "audio/mpeg",
    ogg: "audio/ogg",
    flac: "audio/flac",
    wav: "audio/wav",
    aiff: "audio/aiff",
  }[ext] ?? "application/octet-stream";
}

async function writeAll(handle, chunk) {
  let offset = 0;
  while (offset < chunk.length) {
    const { bytesWritten } = await handle.write(chunk, offset, chunk.length - offset);
    if (bytesWritten <= 0) throw new Error("short_write");
    offset += bytesWritten;
  }
}

export class UploadStore {
  #dir;
  #userMaxBytes;
  #fileMaxBytes;
  #diskFloorBytes;
  #now;
  #entries = new Map();
  #usage = new Map();
  #queue = new KeyedQueue();

  constructor({ dir, userMaxBytes = 5 * 1024 ** 3, fileMaxBytes = 512 * 1024 ** 2, diskFloorBytes = 4 * 1024 ** 3, now = Date.now }) {
    this.#dir = path.join(dir, "uploads");
    this.#userMaxBytes = userMaxBytes;
    this.#fileMaxBytes = fileMaxBytes;
    this.#diskFloorBytes = diskFloorBytes;
    this.#now = now;
  }

  get dir() {
    return this.#dir;
  }

  static keyFor(sub, key) {
    return `${ownerPrefix(sub)}/${key}`;
  }

  async init() {
    await fs.mkdir(path.join(this.#dir, ".tmp"), { recursive: true, mode: 0o750 });
    const now = this.#now();
    const owners = await fs.readdir(this.#dir, { withFileTypes: true }).catch((error) => {
      if (error?.code === "ENOENT") return [];
      throw error;
    });
    for (const owner of owners) {
      if (!owner.isDirectory() || !OWNER_DIR.test(owner.name)) continue;
      const ownerDir = path.join(this.#dir, owner.name);
      const files = await fs.readdir(ownerDir).catch(() => []);
      for (const name of files) {
        const match = /^([A-Za-z0-9][A-Za-z0-9_-]{0,127})\.([a-z0-9]{2,5})$/.exec(name);
        if (!match) continue;
        const file = path.join(ownerDir, name);
        let stat;
        try {
          stat = await fs.lstat(file);
        } catch {
          continue;
        }
        if (!stat.isFile()) continue;
        const key = `${owner.name}/${match[1]}`;
        this.#entries.set(key, {
          owner: owner.name,
          key: match[1],
          file,
          size: stat.size,
          mtimeMs: stat.mtimeMs,
          contentType: contentTypeForExt(match[2]),
        });
        this.#usage.set(owner.name, (this.#usage.get(owner.name) ?? 0) + stat.size);
      }
    }
    await this.#cleanTemp(now);
    return this;
  }

  /** Keeps uploads from ever starving what else runs on this machine. */
  async #diskIsLow(incoming) {
    try {
      const disk = await fs.statfs(this.#dir);
      return Number(disk.bavail) * Number(disk.bsize) - incoming < this.#diskFloorBytes;
    } catch {
      return false;
    }
  }

  async #cleanTemp(now) {
    const tmp = path.join(this.#dir, ".tmp");
    const files = await fs.readdir(tmp).catch(() => []);
    for (const name of files) {
      const file = path.join(tmp, name);
      try {
        const stat = await fs.stat(file);
        if (now - stat.mtimeMs > 60 * 60 * 1000) await fs.unlink(file);
      } catch {
        // Already gone.
      }
    }
  }

  lookup(sub, key) {
    if (!UPLOAD_KEY.test(key)) return null;
    return this.#entries.get(UploadStore.keyFor(sub, key)) ?? null;
  }

  usage(sub) {
    return this.#usage.get(ownerPrefix(sub)) ?? 0;
  }

  store(sub, key, req, options = {}) {
    // One upload at a time per user keeps the byte cap honest under races.
    return this.#queue.run(ownerPrefix(sub), () => this.#store(sub, key, req, options));
  }

  /** Recomputes one account's bytes from the files it actually has, so a
   * concurrent removal is never overwritten by a stale projection. */
  #recount(owner) {
    let total = 0;
    for (const entry of this.#entries.values()) {
      if (entry.owner === owner) total += entry.size;
    }
    this.#usage.set(owner, total);
  }

  async #store(sub, key, req, { contentLength } = {}) {
    if (!UPLOAD_KEY.test(key)) throw new UploadError("invalid_key", 400);
    if (await this.#diskIsLow(0)) throw new UploadError("disk_full", 507);
    const owner = ownerPrefix(sub);
    const identity = UploadStore.keyFor(sub, key);
    const existing = this.#entries.get(identity);
    let declared = contentLength;
    if (declared !== undefined) {
      if (!Number.isSafeInteger(declared) || declared < 0) throw new UploadError("invalid_length", 400);
      if (declared > this.#fileMaxBytes) throw new UploadError("file_too_large", 413);
      const projected = this.usage(sub) - (existing?.size ?? 0) + declared;
      if (projected > this.#userMaxBytes) throw new UploadError("user_limit_reached", 413);
    }

    const tmpDir = path.join(this.#dir, ".tmp");
    await fs.mkdir(tmpDir, { recursive: true, mode: 0o750 });
    await fs.mkdir(path.join(this.#dir, owner), { recursive: true, mode: 0o750 });
    const temp = path.join(tmpDir, `${owner}-${key}-${process.pid}-${Date.now()}.part`);
    let handle;
    let received = 0;
    let head = Buffer.alloc(0);
    let format = null;
    try {
      handle = await fs.open(temp, "wx", 0o600);
      for await (const chunk of req) {
        const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
        received += bytes.length;
        if (received > this.#fileMaxBytes) throw new UploadError("file_too_large", 413);
        if (head.length < 12) {
          head = Buffer.concat([head, bytes.subarray(0, 12 - head.length)]);
          if (head.length >= 12) {
            format = audioFormat(head);
            if (!format) throw new UploadError("not_audio", 415);
          }
        }
        await writeAll(handle, bytes);
      }
      if (received === 0) throw new UploadError("empty_file", 400);
      if (declared !== undefined && received !== declared) throw new UploadError("length_mismatch", 400);
      if (!format && head.length < 12) format = audioFormat(head);
      if (!format) throw new UploadError("not_audio", 415);
      const projected = this.usage(sub) - (existing?.size ?? 0) + received;
      if (projected > this.#userMaxBytes) throw new UploadError("user_limit_reached", 413);
      await handle.sync();
      await handle.close();
      handle = undefined;
      const target = path.join(this.#dir, owner, `${key}.${format.ext}`);
      if (existing && existing.file !== target) await fs.unlink(existing.file).catch(() => {});
      await fs.rename(temp, target);
      const stat = await fs.stat(target);
      const entry = {
        owner,
        key,
        file: target,
        size: stat.size,
        mtimeMs: stat.mtimeMs,
        contentType: format.contentType,
      };
      this.#entries.set(identity, entry);
      this.#recount(owner);
      return entry;
    } catch (error) {
      if (handle) await handle.close().catch(() => {});
      await fs.unlink(temp).catch(() => {});
      throw error instanceof UploadError ? error : new UploadError("upload_failed", 500);
    }
  }

  async remove(sub, key) {
    const identity = UploadStore.keyFor(sub, key);
    const entry = this.#entries.get(identity);
    if (!entry) return false;
    await fs.unlink(entry.file).catch(() => {});
    this.#entries.delete(identity);
    this.#recount(entry.owner);
    return true;
  }

  stats() {
    let bytes = 0;
    for (const total of this.#usage.values()) bytes += total;
    return { bytes, files: this.#entries.size };
  }
}

// Isolated M2 foundation. Not wired to a Worker route or a storage signer.
// Actor identities and queue eligibility must come from a trusted authority.
// settle* / cleanup* are worker-only outcomes, never participant assertions.
export const UPLOAD_LIMITS = Object.freeze({
  fileBytes: 16 * 1024 * 1024, ownerBytes: 64 * 1024 * 1024,
  totalBytes: 128 * 1024 * 1024, identities: 64,
  activePerOwner: 3, partsPerOwner: 2, partBytes: 8 * 1024 * 1024,
  leaseMs: 5 * 60 * 1000, retentionMs: 24 * 60 * 60 * 1000,
  retryMs: 60 * 1000, maxRetryMs: 15 * 60 * 1000,
});
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const HASH = /^[0-9a-f]{64}$/;
const ETAG = /^[!#-~]{1,128}$/;
// Workers multipart receipts are opaque tokens, not object MD5 ETags. The
// local R2 runtime returns 171 characters; preserve them exactly for completion.
const PART_ETAG = /^[!#-~]{1,1024}$/;
const PROFILE = "mp3-44100-stereo";
const ACTIVE = new Set(["reserved", "creating", "uploading", "completing"]);

export class UploadError extends Error {
  constructor(code) { super(code); this.code = code; }
}
function requireThat(value, code) { if (!value) throw new UploadError(code); }
function fields(value, names) {
  requireThat(value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).sort().join(",") === [...names].sort().join(","), "invalid_fields");
}
function integer(value, min, max) { return Number.isSafeInteger(value) && value >= min && value <= max; }
function uuid(value) { return typeof value === "string" && UUID.test(value); }
function hash(value) { return typeof value === "string" && HASH.test(value); }
function etag(value) { return typeof value === "string" && ETAG.test(value); }
function selection(actor, input) {
  fields(input, ["id", "room_id", "occurrence_id", "size", "sha256", "profile", "explicit_selection"]);
  requireThat(uuid(actor) && [input.id, input.room_id, input.occurrence_id].every(uuid)
    && integer(input.size, 1, UPLOAD_LIMITS.fileBytes) && hash(input.sha256)
    && input.profile === PROFILE && input.explicit_selection === true, "invalid_selection");
}
function canonical(input) {
  return JSON.stringify([input.room_id, input.occurrence_id, input.size, input.sha256, input.profile]);
}

export class UploadLedger {
  constructor(storage, { authorize, now = Date.now } = {}) {
    requireThat(typeof authorize === "function" && typeof now === "function", "authority_required");
    this.storage = storage; this.sql = storage.sql; this.authorize = authorize; this.now = now;
    this.sql.exec(`CREATE TABLE IF NOT EXISTS upload_sessions (
      id TEXT PRIMARY KEY, owner TEXT NOT NULL, room_id TEXT NOT NULL, occurrence_id TEXT NOT NULL,
      request TEXT NOT NULL, size INTEGER NOT NULL, retained_bytes INTEGER NOT NULL, sha256 TEXT NOT NULL, profile TEXT NOT NULL,
      state TEXT NOT NULL, generation INTEGER NOT NULL DEFAULT 1, revision INTEGER NOT NULL DEFAULT 1,
      expires_at INTEGER NOT NULL, reason TEXT NOT NULL DEFAULT '',
      create_generation INTEGER, create_pending INTEGER NOT NULL DEFAULT 0, create_result TEXT,
      multipart_id TEXT, complete_generation INTEGER, complete_pending INTEGER NOT NULL DEFAULT 0,
      complete_result TEXT, attempts INTEGER NOT NULL DEFAULT 0, retry_at INTEGER NOT NULL DEFAULT 0)`);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS upload_parts (
      upload_id TEXT NOT NULL, number INTEGER NOT NULL, size INTEGER NOT NULL,
      lease_id TEXT, lease_until INTEGER NOT NULL DEFAULT 0, etag TEXT,
      PRIMARY KEY (upload_id, number))`);
  }
  rows(query, ...args) { return this.sql.exec(query, ...args).toArray(); }
  row(id) {
    requireThat(uuid(id), "unavailable");
    return this.rows("SELECT * FROM upload_sessions WHERE id = ?", id)[0];
  }
  async commit(operation) {
    const result = this.storage.transactionSync(operation);
    // No caller receives an effect or acknowledgement before durable storage.
    await this.storage.sync();
    return result;
  }
  owned(actor, id, operation) {
    const row = this.row(id);
    requireThat(uuid(actor) && row?.owner === actor, "unavailable");
    // Strict true also rejects an accidentally asynchronous authority callback.
    requireThat(this.authorize(actor, row.room_id, row.occurrence_id, operation) === true, "unavailable");
    this.expireRow(row);
    return this.row(id);
  }
  current(row, generation, states) {
    requireThat(row && row.generation === generation, "stale_generation");
    requireThat(states.includes(row.state), "invalid_state");
  }
  view(row) {
    const parts = this.rows("SELECT number,size,etag FROM upload_parts WHERE upload_id = ? ORDER BY number", row.id);
    return {
      id: row.id, state: row.state, generation: row.generation, revision: row.revision,
      size: row.size, expires_at: row.expires_at, reason: row.reason,
      uploaded_bytes: parts.reduce((sum, part) => sum + (part.etag ? part.size : 0), 0),
      parts: parts.map(part => ({ number: part.number, offset: (part.number - 1) * UPLOAD_LIMITS.partBytes,
        size: part.size, complete: Boolean(part.etag) })),
    };
  }
  async admit(actor, input) {
    selection(actor, input);
    return this.commit(() => {
      requireThat(this.authorize(actor, input.room_id, input.occurrence_id, "admit") === true, "unavailable");
      const prior = this.row(input.id);
      if (prior) {
        requireThat(prior.owner === actor, "unavailable");
        requireThat(prior.request === canonical(input), "identity_conflict");
        this.expireRow(prior);
        return this.view(this.row(input.id));
      }
      for (const row of this.rows("SELECT * FROM upload_sessions")) this.expireRow(row);
      const rows = this.rows("SELECT * FROM upload_sessions");
      const retained = rows.filter(row => row.state !== "cancelled");
      requireThat(rows.length < UPLOAD_LIMITS.identities
        && retained.reduce((sum, row) => sum + row.retained_bytes, 0) + input.size <= UPLOAD_LIMITS.totalBytes
        && retained.filter(row => row.owner === actor).reduce((sum, row) => sum + row.retained_bytes, 0) + input.size <= UPLOAD_LIMITS.ownerBytes
        && rows.filter(row => row.owner === actor && ACTIVE.has(row.state)).length < UPLOAD_LIMITS.activePerOwner, "quota_reached");
      requireThat(!retained.some(row => row.owner === actor && row.room_id === input.room_id
        && row.occurrence_id === input.occurrence_id), "occurrence_already_reserved");
      this.sql.exec(`INSERT INTO upload_sessions (id,owner,room_id,occurrence_id,request,size,retained_bytes,sha256,profile,state,expires_at)
        VALUES (?,?,?,?,?,?,?,?,?,'reserved',?)`, input.id, actor, input.room_id, input.occurrence_id,
      canonical(input), input.size, input.size, input.sha256, input.profile, this.now() + UPLOAD_LIMITS.retentionMs);
      const count = Math.ceil(input.size / UPLOAD_LIMITS.partBytes);
      for (let number = 1; number <= count; number++) this.sql.exec(
        "INSERT INTO upload_parts (upload_id,number,size) VALUES (?,?,?)", input.id, number,
        Math.min(UPLOAD_LIMITS.partBytes, input.size - (number - 1) * UPLOAD_LIMITS.partBytes));
      return this.view(this.row(input.id));
    });
  }
  async snapshot(actor, id, identity) {
    return this.commit(() => {
      const row = this.owned(actor, id, "read");
      if (identity !== undefined) {
        fields(identity, ["size", "sha256"]);
        requireThat(identity.size === row.size && identity.sha256 === row.sha256, "file_changed");
      }
      return this.view(row);
    });
  }
  // Claims an effect once. An uncertain creation must be reconciled, never
  // blindly repeated: S3 multipart creation is not idempotent.
  async beginCreate(actor, id, generation) {
    return this.commit(() => {
      const row = this.owned(actor, id, "upload");
      this.current(row, generation, ["reserved", "creating", "uploading", "completing", "ready"]);
      if (row.state !== "reserved") return null;
      this.sql.exec("UPDATE upload_sessions SET state='creating', create_pending=1, create_generation=?, revision=revision+1 WHERE id=?", generation, id);
      return { id, generation, key: `uploads/${id}.mp3`, contentType: "audio/mpeg" };
    });
  }
  // null means independently confirmed that creation made no multipart upload.
  // A timeout or lost response is not that confirmation.
  async settleCreate(id, generation, multipartId) {
    requireThat(multipartId === null || (typeof multipartId === "string" && /^[\x21-\x7e]{1,1024}$/.test(multipartId)), "invalid_storage_result");
    return this.commit(() => {
      const row = this.row(id);
      requireThat(row?.create_generation === generation, "stale_generation");
      this.expireRow(row);
      const current = this.row(id);
      const result = JSON.stringify(multipartId);
      if (current.create_result !== null) { requireThat(current.create_result === result, "storage_result_conflict"); return this.view(current); }
      this.sql.exec(`UPDATE upload_sessions SET create_pending=0, create_result=?, multipart_id=?,
        state=CASE WHEN state='creating' THEN 'uploading' ELSE state END, revision=revision+1 WHERE id=?`, result, multipartId, id);
      if (multipartId === null && this.row(id).state === "uploading") this.cancelRow(this.row(id), "creation_failed");
      return this.view(this.row(id));
    });
  }
  async leasePart(actor, id, generation, number) {
    return this.commit(() => {
      const row = this.owned(actor, id, "upload");
      this.current(row, generation, ["uploading"]);
      const part = this.rows("SELECT * FROM upload_parts WHERE upload_id=? AND number=?", id, number)[0];
      requireThat(Number.isSafeInteger(number) && part, "invalid_part");
      requireThat(!part.etag, "part_already_complete");
      if (part.lease_until > this.now()) return this.leaseView(row, part);
      const active = this.rows(`SELECT COUNT(*) AS count FROM upload_parts p JOIN upload_sessions u ON p.upload_id=u.id
        WHERE u.owner=? AND u.state!='cancelled' AND p.etag IS NULL AND p.lease_until>?`, actor, this.now())[0].count;
      requireThat(active < UPLOAD_LIMITS.partsPerOwner, "part_limit");
      this.sql.exec("UPDATE upload_parts SET lease_id=?, lease_until=? WHERE upload_id=? AND number=?",
        crypto.randomUUID(), Math.min(row.expires_at, this.now() + UPLOAD_LIMITS.leaseMs), id, number);
      this.sql.exec("UPDATE upload_sessions SET revision=revision+1 WHERE id=?", id);
      return this.leaseView(this.row(id), this.rows("SELECT * FROM upload_parts WHERE upload_id=? AND number=?", id, number)[0]);
    });
  }
  leaseView(row, part) {
    return { id: row.id, generation: row.generation, revision: row.revision, number: part.number,
      offset: (part.number - 1) * UPLOAD_LIMITS.partBytes, size: part.size,
      lease_id: part.lease_id, expires_at: part.lease_until };
  }
  // Storage-confirmed part receipt, not a client's claimed byte count or ETag.
  async recordPart(id, generation, input) {
    fields(input, ["number", "lease_id", "size", "etag"]);
    requireThat(uuid(input.lease_id) && typeof input.etag === "string" && PART_ETAG.test(input.etag)
      && integer(input.number, 1, Math.ceil(UPLOAD_LIMITS.fileBytes / UPLOAD_LIMITS.partBytes))
      && integer(input.size, 1, Number.MAX_SAFE_INTEGER), "invalid_storage_result");
    return this.commit(() => {
      const row = this.row(id); if (row) this.expireRow(row);
      this.current(this.row(id), generation, ["uploading", "completing", "ready"]);
      const part = this.rows("SELECT * FROM upload_parts WHERE upload_id=? AND number=?", id, input.number)[0];
      requireThat(part && part.lease_id === input.lease_id, "stale_part");
      if (part.etag) {
        requireThat(part.size === input.size && part.etag === input.etag, "storage_result_conflict");
        return this.view(this.row(id));
      }
      requireThat(this.row(id).state === "uploading", "invalid_state");
      if (part.size !== input.size) {
        const retained = Math.max(row.retained_bytes, Math.min(Number.MAX_SAFE_INTEGER, row.size - part.size + input.size));
        this.sql.exec("UPDATE upload_sessions SET retained_bytes=? WHERE id=?", retained, id);
        this.cancelRow(this.row(id), "part_verification_failed");
        return this.view(this.row(id));
      }
      this.sql.exec("UPDATE upload_parts SET etag=? WHERE upload_id=? AND number=?", input.etag, id, input.number);
      this.sql.exec("UPDATE upload_sessions SET revision=revision+1 WHERE id=?", id);
      return this.view(this.row(id));
    });
  }
  async beginComplete(actor, id, generation) {
    return this.commit(() => {
      const row = this.owned(actor, id, "upload");
      this.current(row, generation, ["uploading", "completing", "ready"]);
      if (row.state !== "uploading") return null;
      const parts = this.rows("SELECT number,etag FROM upload_parts WHERE upload_id=? ORDER BY number", id);
      requireThat(parts.length && parts.every(part => part.etag), "parts_missing");
      this.sql.exec("UPDATE upload_sessions SET state='completing', complete_pending=1, complete_generation=?, revision=revision+1 WHERE id=?", generation, id);
      return { id, generation, key: `uploads/${id}.mp3`, multipart_id: row.multipart_id, parts };
    });
  }
  // A verifier must hash and decode the stored bytes. Client metadata and a
  // multipart ETag are not proof. null means terminal, confirmed absence.
  async settleComplete(id, generation, proof) {
    if (proof !== null) {
      fields(proof, ["size", "sha256", "profile", "etag", "version"]);
      requireThat(integer(proof.size, 1, Number.MAX_SAFE_INTEGER) && hash(proof.sha256)
        && typeof proof.profile === "string" && proof.profile.length <= 64 && etag(proof.etag)
        && typeof proof.version === "string" && /^[\x21-\x7e]{1,128}$/.test(proof.version), "invalid_storage_result");
    }
    return this.commit(() => {
      const row = this.row(id);
      requireThat(row?.complete_generation === generation, "stale_generation");
      this.expireRow(row);
      const current = this.row(id);
      const result = proof === null ? "null" : JSON.stringify([proof.size, proof.sha256, proof.profile, proof.etag, proof.version]);
      if (current.complete_result !== null) { requireThat(current.complete_result === result, "storage_result_conflict"); return this.view(current); }
      this.sql.exec("UPDATE upload_sessions SET complete_pending=0, complete_result=?, retained_bytes=?, revision=revision+1 WHERE id=?",
        result, Math.max(current.retained_bytes, proof?.size ?? 0), id);
      if (current.state === "completing") {
        if (proof && proof.size === current.size && proof.sha256 === current.sha256 && proof.profile === current.profile) {
          this.sql.exec("UPDATE upload_sessions SET state='ready' WHERE id=?", id);
        } else this.cancelRow(this.row(id), "verification_failed");
      }
      return this.view(this.row(id));
    });
  }
  cancelRow(row, reason) {
    if (["cleanup", "cancelled"].includes(row.state)) return;
    // Only never-dispatched reservations can release quota immediately.
    const state = row.state === "reserved" ? "cancelled" : "cleanup";
    this.sql.exec("UPDATE upload_sessions SET state=?, reason=?, generation=generation+1, revision=revision+1, retry_at=? WHERE id=?", state, reason, this.now(), row.id);
  }
  expireRow(row) {
    if (row.expires_at <= this.now()) this.cancelRow(row, "expired");
  }
  async cancel(actor, id, generation) {
    return this.commit(() => {
      const row = this.owned(actor, id, "cancel");
      if (["cleanup", "cancelled"].includes(row.state)) {
        requireThat(generation === row.generation || generation === row.generation - 1, "stale_generation");
      } else {
        this.current(row, generation, ["reserved", "creating", "uploading", "completing", "ready"]);
        this.cancelRow(row, "cancelled");
      }
      return this.view(this.row(id));
    });
  }
  // The original selection permits cancellation to arrive before admission.
  // Reserve its identity without reserving bytes or starting provider work.
  async cancelBeforeAdmission(actor, input) {
    selection(actor, input);
    return this.commit(() => {
      requireThat(this.authorize(actor, input.room_id, input.occurrence_id, "cancel") === true, "unavailable");
      const prior = this.row(input.id);
      if (prior) {
        requireThat(prior.owner === actor, "unavailable");
        requireThat(prior.request === canonical(input), "identity_conflict");
        this.cancelRow(prior, "cancelled");
      } else {
        requireThat(this.rows("SELECT COUNT(*) AS count FROM upload_sessions")[0].count < UPLOAD_LIMITS.identities, "quota_reached");
        this.sql.exec(`INSERT INTO upload_sessions (id,owner,room_id,occurrence_id,request,size,retained_bytes,sha256,profile,state,generation,expires_at,reason)
          VALUES (?,?,?,?,?,?,0,?,?,'cancelled',2,?,'cancelled')`, input.id, actor, input.room_id,
        input.occurrence_id, canonical(input), input.size, input.sha256, input.profile, this.now() + UPLOAD_LIMITS.retentionMs);
      }
      return this.view(this.row(input.id));
    });
  }
  async cleanupTasks() {
    return this.commit(() => {
      for (const row of this.rows("SELECT * FROM upload_sessions")) this.expireRow(row);
      return this.rows("SELECT * FROM upload_sessions WHERE state='cleanup' AND create_pending=0 AND complete_pending=0 AND retry_at<=? ORDER BY retry_at LIMIT 16", this.now())
        .map(row => ({ id: row.id, generation: row.generation, key: `uploads/${row.id}.mp3`, multipart_id: row.multipart_id }));
    });
  }
  async cleanupFailed(id, generation) {
    return this.commit(() => {
      const row = this.row(id); this.current(row, generation, ["cleanup"]);
      const attempts = Math.min(row.attempts + 1, 16);
      this.sql.exec("UPDATE upload_sessions SET attempts=?, retry_at=?, revision=revision+1 WHERE id=?", attempts,
        this.now() + Math.min(UPLOAD_LIMITS.retryMs * 2 ** (attempts - 1), UPLOAD_LIMITS.maxRetryMs), id);
    });
  }
  // Only after abort AND object deletion/absence are confirmed. Never call on
  // timeout. Pending provider operations must first be conclusively reconciled.
  async acknowledgeCleanup(id, generation) {
    return this.commit(() => {
      const row = this.row(id); this.current(row, generation, ["cleanup", "cancelled"]);
      requireThat(!row.create_pending && !row.complete_pending, "storage_outcome_pending");
      if (row.state === "cancelled") return;
      this.sql.exec("UPDATE upload_sessions SET state='cancelled', multipart_id=NULL, retry_at=0, revision=revision+1 WHERE id=?", id);
      // Retain the small identity tombstone and generation, not part handles.
      this.sql.exec("DELETE FROM upload_parts WHERE upload_id=?", id);
    });
  }
}

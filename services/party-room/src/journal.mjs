// Storage-independent write-before-dispatch coordinator. No socket or audio.
import { RoomClient } from "./client.mjs";
const CREATE = Symbol("journal-client");
const ERRORS = new Set(["journal_unavailable", "invalid_journal", "journal_conflict", "journal_identity_conflict"]);

export class JournalError extends Error {
  constructor(code) { super(code); this.code = code; }
}
function requireThat(value, code = "journal_unavailable") { if (!value) throw new JournalError(code); }
export function normalizeJournalError(error) {
  return error instanceof JournalError && ERRORS.has(error.code) ? error : new JournalError("journal_unavailable");
}

export class JournaledRoomClient {
  #client;
  #journal;
  #revision;
  #observed;
  #activity = 0;
  #changed = 1;
  #saved = 0;
  #writing = null;
  #unconfirmed = null;
  #error = null;
  constructor(client, journal, revision, token) {
    requireThat(token === CREATE); this.#client = client; this.#journal = journal; this.#revision = revision;
    this.#observed = client.checkpoint();
  }
  static async open(identity, journal) {
    try {
      requireThat(journal && typeof journal.read === "function" && typeof journal.write === "function");
      const saved = await journal.read();
      requireThat(saved && Object.keys(saved).sort().join(",") === "checkpoint,revision"
        && Number.isSafeInteger(saved.revision) && saved.revision >= 0
        && (saved.revision === 0 ? saved.checkpoint === null : typeof saved.checkpoint === "string"), "invalid_journal");
      let client;
      try { client = saved.checkpoint === null ? new RoomClient(identity) : RoomClient.restore(identity, saved.checkpoint); }
      catch { throw new JournalError("invalid_journal"); }
      return new JournaledRoomClient(client, journal, saved.revision, CREATE);
    } catch (error) { throw normalizeJournalError(error); }
  }
  #change(method, ...args) {
    const result = this.#client[method](...args), checkpoint = this.#client.checkpoint(); this.#activity++;
    if (checkpoint !== this.#observed) { this.#observed = checkpoint; this.#changed++; }
    return result;
  }
  beginConnection() { return this.#change("beginConnection"); }
  disconnect(generation) { return this.#change("disconnect", generation); }
  receiveSnapshot(...args) { return this.#change("receiveSnapshot", ...args); }
  receiveReceipt(...args) { return this.#change("receiveReceipt", ...args); }
  receiveFailure(...args) { return this.#change("receiveFailure", ...args); }
  enqueue(...args) { return this.#change("enqueue", ...args); }
  enqueueTransport(...args) { return this.#change("enqueueTransport", ...args); }
  discard(id) { return this.#change("discard", id); }
  retryRejected(id) { return this.#change("retryRejected", id); }
  acceptUncertainOutcome(id) { return this.#change("acceptUncertainOutcome", id); }
  view() {
    return { ...this.#client.view(), journal: { revision: this.#revision,
      state: this.#writing ? "saving" : this.#error ? "failed" : this.#saved === this.#changed ? "saved" : "dirty",
      error: this.#error } };
  }
  flush() {
    if (this.#writing) return this.#writing;
    this.#writing = this.#persist().finally(() => { this.#writing = null; });
    return this.#writing;
  }
  async #persist() {
    try {
      while (this.#saved < this.#changed) {
        if (!this.#unconfirmed) this.#unconfirmed = { change: this.#changed,
          request: { expected_revision: this.#revision, checkpoint: this.#observed } };
        const pending = this.#unconfirmed;
        // Retry an unknown write outcome with exactly its original body before
        // persisting newer changes. The adapter must implement idempotent CAS.
        const revision = await this.#journal.write({ ...pending.request });
        requireThat(Number.isSafeInteger(revision) && revision === pending.request.expected_revision + 1);
        this.#revision = revision; this.#saved = pending.change; this.#unconfirmed = null;
      }
      this.#error = null;
    } catch (error) { const normalized = normalizeJournalError(error); this.#error = normalized.code; throw normalized; }
  }
  async outbound() {
    const packet = this.#change("outbound"), version = this.#activity;
    await this.flush();
    // A disconnect, new snapshot or resolved receipt during the disk wait must
    // not release an obsolete packet to a former connection. Ask again.
    return this.#activity === version ? packet : null;
  }
}

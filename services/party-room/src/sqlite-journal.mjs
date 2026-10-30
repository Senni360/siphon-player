// Local spike adapter for a dedicated node:sqlite DatabaseSync, owned by the
// caller. Its blocking transactions must never run on a native UI thread.
import { RoomClient } from "./client.mjs";
import { JournalError, normalizeJournalError } from "./journal.mjs";

function requireThat(value, code) { if (!value) throw new JournalError(code); }
export class SqliteRoomJournal {
  constructor(database, identity, { create = false } = {}) {
    this.db = database; this.identity = { roomId: identity.roomId, actorId: identity.actorId };
    try {
      new RoomClient(this.identity);
      requireThat(typeof create === "boolean", "invalid_journal");
      if (create) requireThat(database.prepare("SELECT COUNT(*) AS n FROM sqlite_schema WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%'").get().n === 0, "invalid_journal");
      else this.row();
      database.exec("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;");
      this.transaction(() => {
        if (create) {
          database.exec(`CREATE TABLE room_client_journal (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1), room_id TEXT NOT NULL, actor_id TEXT NOT NULL,
            revision INTEGER NOT NULL, checkpoint TEXT)`);
          database.prepare("INSERT INTO room_client_journal VALUES(1,?,?,0,NULL)").run(identity.roomId, identity.actorId);
        }
        this.row();
      });
    } catch (error) { throw normalizeJournalError(error); }
  }
  transaction(operation) {
    this.db.exec("BEGIN IMMEDIATE");
    try { const result = operation(); this.db.exec("COMMIT"); return result; }
    catch (error) { try { this.db.exec("ROLLBACK"); } catch { /* Preserve the original normalized error. */ } throw error; }
  }
  row() {
    const row = this.db.prepare("SELECT * FROM room_client_journal WHERE singleton=1").get();
    requireThat(row && row.room_id === this.identity.roomId && row.actor_id === this.identity.actorId, "journal_identity_conflict");
    requireThat(Number.isSafeInteger(row.revision) && row.revision >= 0
      && (row.revision === 0 ? row.checkpoint === null : typeof row.checkpoint === "string"), "invalid_journal");
    return row;
  }
  async read() {
    try { const row = this.row(); return { revision: row.revision, checkpoint: row.checkpoint }; }
    catch (error) { throw normalizeJournalError(error); }
  }
  async write(request) {
    try {
      requireThat(request && Object.keys(request).sort().join(",") === "checkpoint,expected_revision"
        && Number.isSafeInteger(request.expected_revision) && request.expected_revision >= 0
        && request.expected_revision < Number.MAX_SAFE_INTEGER, "invalid_journal");
      try { RoomClient.restore(this.identity, request.checkpoint); } catch { throw new JournalError("invalid_journal"); }
      return this.transaction(() => {
        const row = this.row();
        if (row.revision === request.expected_revision + 1 && row.checkpoint === request.checkpoint) return row.revision;
        requireThat(row.revision === request.expected_revision, "journal_conflict");
        const revision = row.revision + 1;
        this.db.prepare("UPDATE room_client_journal SET revision=?,checkpoint=? WHERE singleton=1").run(revision, request.checkpoint);
        return revision;
      });
    } catch (error) { throw normalizeJournalError(error); }
  }
}

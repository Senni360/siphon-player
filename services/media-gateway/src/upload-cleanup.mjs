// Isolated trusted executor. No route, scheduler, credentials or upload body.
// Only the ledger may supply tasks; participants cannot request arbitrary keys.
const TASK_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

export class CleanupError extends Error {
  constructor() { super("Upload cleanup could not persist its state"); }
}

function validTask(task) {
  return task && typeof task.id === "string" && TASK_ID.test(task.id)
    && task.key === `uploads/${task.id}.mp3`
    && Number.isSafeInteger(task.generation) && task.generation >= 1
    && (task.multipart_id === null || (typeof task.multipart_id === "string"
      && /^[\x21-\x7e]{1,1024}$/.test(task.multipart_id)));
}

// Workers R2 reports this documented code at the end of the error message.
// Do not treat arbitrary 404s, timeouts, access failures or resumed handles as
// evidence that an upload no longer exists. Never log the provider message.
function noSuchUpload(error) {
  return error instanceof Error && /\(10024\)$/.test(error.message);
}

export class UploadCleanup {
  #ledger;
  #bucket;
  #running;

  constructor(ledger, bucket) {
    this.#ledger = ledger;
    this.#bucket = bucket;
  }

  // Coalesce alarm/cron overlap within one owning instance. Across restarts,
  // stable keys and idempotent abort/delete/ack make repeated work safe. The
  // future Durable Object must own a single executor and schedule its retries.
  run() {
    if (!this.#running) {
      this.#running = this.#sweep().finally(() => { this.#running = undefined; });
    }
    return this.#running;
  }

  async #sweep() {
    try {
      const tasks = await this.#ledger.cleanupTasks();
      if (!Array.isArray(tasks) || tasks.length > 16 || !tasks.every(validTask)) throw new CleanupError();
      const result = { attempted: tasks.length, cleaned: 0, retry: 0 };
      for (const task of tasks) {
        try {
          if (task.multipart_id !== null) {
            const upload = this.#bucket.resumeMultipartUpload(task.key, task.multipart_id);
            try { await upload.abort(); }
            catch (error) { if (!noSuchUpload(error)) throw error; }
          }
          // The ledger withholds tasks while create/complete outcomes are
          // pending. These keys are unique and never reused by later uploads.
          await this.#bucket.delete(task.key);
          if (await this.#bucket.head(task.key) !== null) throw new Error("not_absent");
        } catch {
          await this.#ledger.cleanupFailed(task.id, task.generation);
          result.retry++;
          continue;
        }
        await this.#ledger.acknowledgeCleanup(task.id, task.generation);
        result.cleaned++;
      }
      return result;
    } catch {
      // Database, validation and unexpected executor failures must not leak
      // provider handles, object keys, SQL parameters or credentials.
      throw new CleanupError();
    }
  }
}

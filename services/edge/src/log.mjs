// Logging is deliberately boring: an event name plus small, named fields.
// Callers never pass a request URL, a token, a source link or a file name.
// These helpers exist so redaction is a shared rule rather than a promise.

const TOKEN_PARAM = /([?&](?:token|access_token|authorization)=)[^&]*/gi;
const SECRET_PREFIX = /\b(e1|Bearer)\s+[A-Za-z0-9._~+/-]+=*/g;

/** Drops a query string and fragment; keeps scheme, host and path only. */
export function safeUrl(raw) {
  if (typeof raw !== "string") return "";
  try {
    const url = new URL(raw);
    return `${url.protocol}//${url.host}${url.pathname}`;
  } catch {
    return "";
  }
}

/** A last-resort scrubber for text that must pass through a log line. */
export function redact(text) {
  return String(text)
    .replace(TOKEN_PARAM, "$1<redacted>")
    .replace(SECRET_PREFIX, "$1 <redacted>");
}

const LEVELS = { debug: 10, info: 20, warn: 30, error: 40, silent: 100 };

export function createLogger({ level = "info", sink = console, now = Date.now } = {}) {
  const threshold = LEVELS[level] ?? LEVELS.info;
  function emit(name, fields = {}) {
    const line = { at: new Date(now()).toISOString(), event: name };
    for (const [key, value] of Object.entries(fields)) {
      if (value === undefined) continue;
      if (typeof value === "string") line[key] = redact(value);
      else if (typeof value === "number" || typeof value === "boolean" || value === null) line[key] = value;
    }
    try {
      sink.log(JSON.stringify(line));
    } catch {
      // A broken log sink must never take the process down.
    }
  }
  return {
    debug: (name, fields) => threshold <= LEVELS.debug && emit(name, fields),
    info: (name, fields) => threshold <= LEVELS.info && emit(name, fields),
    warn: (name, fields) => threshold <= LEVELS.warn && emit(name, fields),
    error: (name, fields) => threshold <= LEVELS.error && emit(name, fields),
  };
}

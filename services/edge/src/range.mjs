// Single-range parsing only. A multi-range request cannot be served as one
// stream, so it is refused rather than silently answered with the whole file.

/**
 * Returns null when there is no Range header, `{start,end}` for a satisfiable
 * single range, and `{unsatisfiable: true}` for anything else.
 */
export function parseRange(header, size) {
  if (header === undefined || header === null || header === "") return null;
  if (typeof header !== "string" || !Number.isSafeInteger(size) || size < 0) {
    return { unsatisfiable: true };
  }
  const value = header.trim();
  if (!value.toLowerCase().startsWith("bytes=")) return { unsatisfiable: true };
  const spec = value.slice(6).trim();
  if (spec.length === 0 || spec.includes(",")) return { unsatisfiable: true };
  const match = /^(\d*)-(\d*)$/.exec(spec);
  if (!match) return { unsatisfiable: true };
  const [, startText, endText] = match;
  if (startText === "" && endText === "") return { unsatisfiable: true };

  if (startText === "") {
    // bytes=-N: the last N bytes.
    const suffix = Number(endText);
    if (!Number.isSafeInteger(suffix) || suffix <= 0) return { unsatisfiable: true };
    if (size === 0) return { unsatisfiable: true };
    const start = Math.max(0, size - suffix);
    return { start, end: size - 1 };
  }

  const start = Number(startText);
  if (!Number.isSafeInteger(start) || start < 0) return { unsatisfiable: true };
  if (size === 0 || start >= size) return { unsatisfiable: true };
  if (endText === "") return { start, end: size - 1 };
  const end = Number(endText);
  if (!Number.isSafeInteger(end) || end < start) return { unsatisfiable: true };
  return { start, end: Math.min(end, size - 1) };
}

export function contentRange(start, end, size) {
  return `bytes ${start}-${end}/${size}`;
}

export function unsatisfiedRange(size) {
  return `bytes */${size}`;
}

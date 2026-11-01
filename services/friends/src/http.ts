import { httpsUrl, sourceUrl } from './urls.js';

export class HttpError extends Error { constructor(public status: number, message: string, public retryAfter?: number) { super(message); } }
export const fail = (status: number, message: string, retryAfter?: number): never => { throw new HttpError(status, message, retryAfter); };
export const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), {
  status, headers: { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' },
});
export function text(value: unknown, name: string, max: number, optional = false): string {
  if (optional && (value === undefined || value === null)) return '';
  if (typeof value !== 'string' || (!optional && !value.trim()) || value.length > max || /[\x00-\x1f\x7f]/.test(value)) fail(400, `Invalid ${name}`);
  return (value as string).trim();
}
export function https(value: unknown): URL {
  return httpsUrl(value) ?? fail(400, 'Invalid URL');
}
export function validateUrl(value: unknown, source: unknown): string {
  return sourceUrl(value, source) ?? fail(400, 'URL does not match a supported source and playable item');
}
export async function body(request: Request, signal: AbortSignal, max = 16384): Promise<Record<string, unknown>> {
  if (request.headers.get('content-type')?.split(';')[0].trim().toLowerCase() !== 'application/json') fail(415, 'Use application/json');
  if (Number(request.headers.get('content-length')) > max) fail(413, 'Request too large');
  const reader = request.body?.getReader();
  if (!reader) return fail(400, 'Expected JSON object');
  let size = 0;
  const chunks: Uint8Array[] = [];
  const cancel = () => { void reader.cancel().catch(() => {}); };
  signal.addEventListener('abort', cancel, { once: true });
  try {
    signal.throwIfAborted();
    while (true) {
      const part = await reader.read();
      signal.throwIfAborted();
      if (part.done) break;
      size += part.value.length;
      if (size > max) { await reader.cancel(); return fail(413, 'Request too large'); }
      chunks.push(part.value);
    }
  } finally { signal.removeEventListener('abort', cancel); reader.releaseLock(); }
  try {
    const value: unknown = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    if (value && typeof value === 'object' && !Array.isArray(value)) return value as Record<string, unknown>;
  } catch { /* fixed error, never echo input */ }
  return fail(400, 'Expected JSON object');
}

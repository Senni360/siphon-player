import { createHash, createHmac, timingSafeEqual } from 'node:crypto';
import { readFileSync } from 'node:fs';
import type { Storage } from './storage.js';

// Encore on a phone, at its own address, behind a short code. The code only keeps
// strangers from the door: every request that touches an account still needs that
// account's own secret. It sits in the deployment's settings, never in this repo.

export const WEB_HOST = 'encore.senni.dev';
const COOKIE = 'encore_gate';
// The gate stays open to whoever keeps coming back: the cookie is good for
// about ninety days and slides forward on use, so a phone used at least every
// ninety days never has to type the code again. A changed code locks everyone
// out, always.
const KEEP_SECONDS = 90 * 86400;
/** Once a cookie is older than its first few days, using it brings a fresh one. */
const SLIDE_AFTER_SECONDS = 3 * 86400;
/** A passing cookie with less than this left is renewed on use. */
const RENEW_WITHIN_MS = (KEEP_SECONDS - SLIDE_AFTER_SECONDS) * 1000;
const ATTEMPTS_PER_HOUR = 20;

/** Spacing and case don't matter on a phone's keyboard. */
export const normalized = (code: string) => code.normalize('NFKC').toLowerCase().replace(/\s+/g, '');

function sign(until: number, code: string): string {
  return createHmac('sha256', `encore-gate:${normalized(code)}`).update(String(until)).digest('hex');
}

export function gateCookie(code: string, now: number): string {
  const until = now + KEEP_SECONDS * 1000;
  return `${COOKIE}=${until}.${sign(until, code)}; Path=/; Max-Age=${KEEP_SECONDS}; HttpOnly; Secure; SameSite=Lax`;
}

/** The moment a passing gate cookie expires, or null when there is none. */
export function gateUntil(cookies: string | null | undefined, code: string, now: number): number | null {
  const found = cookies?.split(';').map(part => part.trim()).find(part => part.startsWith(`${COOKIE}=`));
  const [until, signature] = found?.slice(COOKIE.length + 1).split('.') ?? [];
  if (!until || !signature || !/^\d+$/.test(until) || Number(until) <= now) return null;
  const expected = Buffer.from(sign(Number(until), code));
  const given = Buffer.from(signature);
  return expected.length === given.length && timingSafeEqual(expected, given) ? Number(until) : null;
}

export function passedGate(cookies: string | null | undefined, code: string, now: number): boolean {
  return gateUntil(cookies, code, now) !== null;
}

export function codeMatches(given: string, code: string): boolean {
  const digest = (text: string) => createHash('sha256').update(normalized(text)).digest();
  return !!normalized(code) && timingSafeEqual(digest(given), digest(code));
}

const CONTENT: Record<string, string> = {
  '/app.js': 'text/javascript; charset=utf-8',
  '/app.css': 'text/css; charset=utf-8',
  '/manifest.webmanifest': 'application/manifest+json',
  '/icon.svg': 'image/svg+xml',
  '/icon-180.png': 'image/png',
  '/icon-192.png': 'image/png',
  '/icon-512.png': 'image/png',
};

const files = new Map<string, Buffer>();
function file(name: string): Buffer {
  let found = files.get(name);
  if (!found) {
    found = readFileSync(new URL(`../assets/web/${name}`, import.meta.url));
    files.set(name, found);
  }
  return found;
}

/**
 * Images come from wherever a song's art is. Nothing else loads from elsewhere, and
 * only Spotify's own sign-in and API are spoken to from the page.
 */
const HEADERS = {
  'Content-Security-Policy': "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' https: data:; media-src https:; font-src 'self'; connect-src 'self' https://accounts.spotify.com https://api.spotify.com; manifest-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
  'X-Content-Type-Options': 'nosniff',
  'Referrer-Policy': 'no-referrer',
};

export interface WebAnswer { status: number; headers: Record<string, string>; body: string | Buffer }

const page = (status: number, name: string, extra: Record<string, string> = {}): WebAnswer => ({
  status,
  headers: { ...HEADERS, 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store', ...extra },
  body: file(name),
});
/** The page that asks for the code: the front door for a stranger, or a regular
 *  whose cookie ran out because they were away for the whole of its life. */
const deferringGate = (path: string) => page(path === '/' ? 200 : 401, 'gate.html');

/** The answer to a browser on the web address, or null when it's the API's to answer. */
export async function webAnswer(
  method: string, url: string, cookies: string | undefined, input: () => Promise<unknown>,
  code: string, storage: Pick<Storage, 'limit'>, ip: string, now = Date.now(),
): Promise<WebAnswer | null> {
  const path = new URL(url, 'http://localhost').pathname;
  if (path === '/api' || path.startsWith('/api/')) return null;
  const plain = (status: number, value: unknown, extra: Record<string, string> = {}): WebAnswer => ({
    status, headers: { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', ...extra }, body: JSON.stringify(value),
  });
  if (path === '/gate' && method === 'POST') {
    if (!code) return plain(503, { error: 'Not set up yet' });
    const hash = createHash('sha256').update(ip).digest('hex');
    if (!await storage.limit(`gate:${hash}`, ATTEMPTS_PER_HOUR, 3600)) return plain(429, { error: 'Too many tries. Wait a while.' });
    const sent = await input().catch(() => null) as { code?: unknown } | null;
    if (typeof sent?.code !== 'string' || sent.code.length > 200 || !codeMatches(sent.code, code)) return plain(403, { error: "That's not the code." });
    return plain(200, { ok: true }, { 'Set-Cookie': gateCookie(code, now) });
  }
  if (method !== 'GET' && method !== 'HEAD') return plain(405, { error: 'Not allowed' });
  // The gate needs its own look and its one script to work before the code is in.
  if (path === '/gate.js') return { status: 200, headers: { ...HEADERS, 'Content-Type': CONTENT['/app.js'], 'Cache-Control': 'no-store' }, body: file('gate.js') };
  if (path === '/app.css' || path === '/icon.svg' || path === '/manifest.webmanifest' || /^\/icon-\d+\.png$/.test(path) && CONTENT[path]) return { status: 200, headers: { ...HEADERS, 'Content-Type': CONTENT[path], 'Cache-Control': 'no-cache' }, body: file(path.slice(1)) };
  if (!code) return deferringGate(path);
  const until = gateUntil(cookies, code, now);
  if (until === null) return deferringGate(path);
  // Used, the gate slides forward: an old-style long cookie becomes the
  // sliding kind at once, and a sliding one older than its first few days is
  // renewed, so a regular never has to type the code again.
  const stale = until - now > KEEP_SECONDS * 1000 || until - now < RENEW_WITHIN_MS;
  const fresh: Record<string, string> = stale ? { 'Set-Cookie': gateCookie(code, now) } : {};
  if (CONTENT[path]) return { status: 200, headers: { ...HEADERS, 'Content-Type': CONTENT[path], 'Cache-Control': 'no-cache', ...fresh }, body: file(path.slice(1)) };
  // The app routes its own screens, and Spotify returns to /callback.
  return page(200, 'index.html', fresh);
}

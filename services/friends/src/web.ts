import { createHash, createHmac, timingSafeEqual } from 'node:crypto';
import { readFileSync } from 'node:fs';
import type { Storage } from './storage.js';

// Encore on a phone, at its own address, behind a short code. The code only keeps
// strangers from the door: every request that touches an account still needs that
// account's own secret. It sits in the deployment's settings, never in this repo.

export const WEB_HOST = 'encore.senni.dev';
const COOKIE = 'encore_gate';
const KEEP_SECONDS = 365 * 86400;
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

export function passedGate(cookies: string | null | undefined, code: string, now: number): boolean {
  const found = cookies?.split(';').map(part => part.trim()).find(part => part.startsWith(`${COOKIE}=`));
  const [until, signature] = found?.slice(COOKIE.length + 1).split('.') ?? [];
  if (!until || !signature || !/^\d+$/.test(until) || Number(until) <= now) return false;
  const expected = Buffer.from(sign(Number(until), code));
  const given = Buffer.from(signature);
  return expected.length === given.length && timingSafeEqual(expected, given);
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

const page = (status: number, name: string): WebAnswer => ({
  status,
  headers: { ...HEADERS, 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' },
  body: file(name),
});

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
  if (path === '/app.css' || path === '/icon.svg') return { status: 200, headers: { ...HEADERS, 'Content-Type': CONTENT[path], 'Cache-Control': 'no-cache' }, body: file(path.slice(1)) };
  if (!code || !passedGate(cookies, code, now)) return page(path === '/' ? 200 : 401, 'gate.html');
  if (CONTENT[path]) return { status: 200, headers: { ...HEADERS, 'Content-Type': CONTENT[path], 'Cache-Control': 'no-cache' }, body: file(path.slice(1)) };
  // The app routes its own screens, and Spotify returns to /callback.
  return page(200, 'index.html');
}

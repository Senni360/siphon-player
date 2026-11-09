import { readFileSync } from 'node:fs';

// Encore on a phone, at its own address. The page itself is public: it shows
// nothing until a phone is linked, and every call that touches an account still
// needs that account's own device secret. The eight-letter link code is the only
// door, and it is short-lived and used once.

export const WEB_HOST = 'encore.senni.dev';

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
 * Images come from wherever a song's art is. Nothing else loads from elsewhere.
 * The page speaks to Spotify's own sign-in and API, and to the always-on edge
 * after the group service has given it a short-lived token.
 */
const HEADERS = {
  'Content-Security-Policy': "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' https: data:; media-src https:; font-src 'self'; connect-src 'self' https://accounts.spotify.com https://api.spotify.com https://edge.encore.senni.dev wss://edge.encore.senni.dev; manifest-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
  'X-Content-Type-Options': 'nosniff',
  'Referrer-Policy': 'no-referrer',
};

export interface WebAnswer { status: number; headers: Record<string, string>; body: string | Buffer }

const page = (status: number, name: string): WebAnswer => ({
  status,
  headers: { ...HEADERS, 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' },
  body: file(name),
});
const plain = (status: number, value: unknown): WebAnswer => ({
  status, headers: { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }, body: JSON.stringify(value),
});

/** The answer to a browser on the web address, or null when it's the API's to answer. */
export function webAnswer(method: string, url: string): WebAnswer | null {
  const path = new URL(url, 'http://localhost').pathname;
  if (path === '/api' || path.startsWith('/api/')) return null;
  if (method !== 'GET' && method !== 'HEAD') return plain(405, { error: 'Not allowed' });
  const type = CONTENT[path];
  if (type) return { status: 200, headers: { ...HEADERS, 'Content-Type': type, 'Cache-Control': 'no-cache' }, body: file(path.slice(1)) };
  // The app routes its own screens, and Spotify returns to /callback.
  return page(200, 'index.html');
}

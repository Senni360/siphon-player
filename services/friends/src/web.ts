import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';

// Encore on a phone, at its own address. The page itself is public: it shows
// nothing until a phone is linked, and every call that touches an account still
// needs that account's own device secret. The eight-letter link code is the only
// door, and it is short-lived and used once.

export const WEB_HOST = 'encore.senni.dev';

const CONTENT: Record<string, string> = {
  '/app.js': 'text/javascript; charset=utf-8',
  '/app.css': 'text/css; charset=utf-8',
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

// Every native module under js/ is part of the shell. Reading the directory
// keeps this list in step with the files: a new module is served, versioned and
// precached without another edit here.
const JS_FILES = readdirSync(new URL('../assets/web/js', import.meta.url), { recursive: true, encoding: 'utf8' })
  .filter(name => name.endsWith('.js'))
  .map(name => name.replaceAll('\\', '/'))
  .sort()
  .map(name => `js/${name}`);
const MODULES = new Set(JS_FILES);

/** A stable, short name for a piece of shell content. */
const hashOf = (body: string | Buffer): string => createHash('sha256').update(body).digest('hex').slice(0, 12);

// The app and every module it imports share one version, because each module's
// imports name that version: a change anywhere is a new URL everywhere, never
// old bytes under a URL a browser kept for good.
let build: string | null = null;
function buildVersion(): string {
  if (!build) build = hashOf([file('app.js'), ...JS_FILES.map(name => file(name))].map(body => body.toString('utf8')).join('\n'));
  return build;
}

const versionOf = (name: string): string => (name === 'app.js' || MODULES.has(name) ? buildVersion() : hashOf(file(name)));

/**
 * Module sources name each other without a version. Stamp this build's version
 * into every relative import, so the browser asks for an immutable URL.
 */
const MODULE_IMPORT = /(\bfrom\s*|\bimport\s*)(['"])(\.{1,2}\/[^'"]+?\.js)\2/g;
function stamped(name: string): string {
  return file(name).toString('utf8').replace(MODULE_IMPORT,
    (_match: string, lead: string, quote: string, specifier: string) => `${lead}${quote}${specifier}?v=${buildVersion()}${quote}`);
}

/** What a browser receives: module sources have their imports versioned. */
const bodyOf = (name: string): string | Buffer => (name === 'app.js' || MODULES.has(name) ? stamped(name) : file(name));

/**
 * A shell URL a browser may keep for good: its version changes whenever the
 * bytes do, so an update is a new URL and a repeat open transfers nothing.
 */
function asset(path: string): string {
  if (path === '/manifest.webmanifest') return `${path}?v=${hashOf(manifest())}`;
  return `${path}?v=${versionOf(path.slice(1))}`;
}

/** The page, pointing at this build's assets. */
function html(): string {
  return file('index.html').toString('utf8')
    .replaceAll('href="/app.css"', `href="${asset('/app.css')}"`)
    .replaceAll('src="/app.js"', `src="${asset('/app.js')}"`)
    .replaceAll('href="/manifest.webmanifest"', `href="${asset('/manifest.webmanifest')}"`)
    .replaceAll('href="/icon.svg"', `href="${asset('/icon.svg')}"`)
    .replaceAll('href="/icon-180.png"', `href="${asset('/icon-180.png')}"`);
}

/** The manifest keeps its shape in a file; its icon URLs carry their version. */
function manifest(): string {
  return file('manifest.webmanifest').toString('utf8')
    .replaceAll('"/icon-192.png"', `"${asset('/icon-192.png')}"`)
    .replaceAll('"/icon-512.png"', `"${asset('/icon-512.png')}"`)
    .replaceAll('"/icon.svg"', `"${asset('/icon.svg')}"`);
}

/** Everything an offline open needs, at this build's URLs. */
function shell(): string[] {
  return ['/', asset('/app.css'), asset('/app.js'), ...JS_FILES.map(name => asset(`/${name}`)),
    asset('/manifest.webmanifest'), asset('/icon.svg'), asset('/icon-180.png'), asset('/icon-192.png'), asset('/icon-512.png')];
}

/** The worker itself, with this build's version and shell baked in. */
function serviceWorker(): string {
  const urls = shell();
  return file('sw.js').toString('utf8')
    .replace('__VERSION__', hashOf(urls.join('\n') + html()))
    .replace('__SHELL__', JSON.stringify(urls));
}

/**
 * Images come from wherever a song's art is. Nothing else loads from elsewhere.
 * The page speaks to Spotify's own sign-in and API, and to the always-on edge
 * after the group service has given it a short-lived token.
 */
const HEADERS = {
  'Content-Security-Policy': "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' https: data:; media-src https:; font-src 'self'; connect-src 'self' https://accounts.spotify.com https://api.spotify.com https://edge.encore.senni.dev wss://edge.encore.senni.dev; manifest-src 'self'; worker-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
  'X-Content-Type-Options': 'nosniff',
  'Referrer-Policy': 'no-referrer',
};

const IMMUTABLE = 'public, max-age=31536000, immutable';
const REVALIDATE = 'no-cache';

export interface WebAnswer { status: number; headers: Record<string, string>; body: string | Buffer }

const matches = (given: string, tag: string): boolean =>
  given.split(',').some(value => value.trim().replace(/^W\//, '') === tag);

/** A body with its own ETag; a matching If-None-Match answers 304 and no body. */
function answer(body: string | Buffer, type: string, cache: string, ifNoneMatch?: string): WebAnswer {
  const tag = `"${hashOf(body)}"`;
  const headers = { ...HEADERS, 'Content-Type': type, 'Cache-Control': cache, ETag: tag };
  return ifNoneMatch && matches(ifNoneMatch, tag) ? { status: 304, headers, body: '' } : { status: 200, headers, body };
}

const plain = (status: number, value: unknown): WebAnswer => ({
  status, headers: { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }, body: JSON.stringify(value),
});

/** The answer to a browser on the web address, or null when it's the API's to answer. */
export function webAnswer(method: string, url: string, ifNoneMatch?: string): WebAnswer | null {
  const parsed = new URL(url, 'http://localhost');
  const path = parsed.pathname;
  if (path === '/api' || path.startsWith('/api/')) return null;
  if (method !== 'GET' && method !== 'HEAD') return plain(405, { error: 'Not allowed' });
  // The app routes its own screens, and Spotify returns to /callback.
  if (path === '/sw.js') return answer(serviceWorker(), 'text/javascript; charset=utf-8', REVALIDATE, ifNoneMatch);
  if (path === '/manifest.webmanifest') return answer(manifest(), 'application/manifest+json', REVALIDATE, ifNoneMatch);
  const type = CONTENT[path] ?? (MODULES.has(path.slice(1)) ? 'text/javascript; charset=utf-8' : undefined);
  if (type) {
    const immutable = parsed.searchParams.get('v') === versionOf(path.slice(1));
    return answer(bodyOf(path.slice(1)), type, immutable ? IMMUTABLE : REVALIDATE, ifNoneMatch);
  }
  return answer(html(), 'text/html; charset=utf-8', REVALIDATE, ifNoneMatch);
}

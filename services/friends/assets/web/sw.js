// Encore's service worker. It keeps the shell for an offline open and stays out
// of the way of everything else: no API calls, no other origin, and no audio or
// range requests. A new version waits until the page offers a refresh; nothing
// reloads under the listener.
'use strict';

const VERSION = '__VERSION__';
const SHELL = __SHELL__;
const CACHE = `encore-shell-${VERSION}`;
const PATHS = new Set(SHELL.map(url => new URL(url, self.location.origin).pathname));

self.addEventListener('install', event => {
  // cache: 'reload' so a stale HTTP cache never gets baked into this version.
  event.waitUntil(caches.open(CACHE).then(cache => cache.addAll(
    SHELL.map(url => new Request(url, { cache: 'reload' })),
  )));
});

self.addEventListener('activate', event => {
  event.waitUntil((async () => {
    for (const key of await caches.keys()) if (key !== CACHE) await caches.delete(key);
    // The page that installed this version can use it at once; a refresh is
    // still the page's own choice.
    await self.clients.claim();
  })());
});

self.addEventListener('message', event => {
  if (event.data?.type === 'skip-waiting') self.skipWaiting();
});

self.addEventListener('fetch', event => {
  const request = event.request;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  // The API, Spotify, artwork and audio all keep the network to themselves.
  if (url.origin !== self.location.origin) return;
  if (url.pathname === '/api' || url.pathname.startsWith('/api/')) return;
  if (request.headers.has('range')) return;
  if (request.destination === 'audio' || request.destination === 'video') return;

  if (request.mode === 'navigate') {
    // Each open asks the network, so a new build shows up; offline, the last
    // good shell opens instead.
    event.respondWith(fetch(request).catch(async () => (await caches.match('/')) ?? Response.error()));
    return;
  }
  // Only the precached shell is served from the cache. Anything else is not ours.
  if (!PATHS.has(url.pathname)) return;
  event.respondWith((async () => (await caches.match(request, { ignoreSearch: true })) ?? fetch(request))());
});

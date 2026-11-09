import assert from 'node:assert/strict';
import { readdirSync } from 'node:fs';
import test from 'node:test';
import { webAnswer, type WebAnswer } from '../src/web.js';

// The phone page at encore.senni.dev. There is no door code any more: the shell
// is public, and only the account's own device secret reaches anything private.
const get = (path: string, method = 'GET', ifNoneMatch?: string): WebAnswer =>
  webAnswer(method, path, ifNoneMatch) as WebAnswer;

test('the shell is public, with no gate and no cookie', () => {
  for (const path of ['/', '/link', '/callback', '/library', '/anything']) {
    const answer = get(path);
    assert.equal(answer.status, 200, path);
    assert.match(String(answer.body), /id="app"/, path);
    assert.equal(answer.headers['Set-Cookie'], undefined, path);
    assert.doesNotMatch(String(answer.body), /encore-gate/, path);
  }
  assert.equal(get('/app.js').status, 200);
  assert.equal(get('/app.css').status, 200);
  assert.equal(get('/manifest.webmanifest').status, 200);
});

test('the old gate route and its scripts are gone', () => {
  assert.equal(get('/gate', 'POST').status, 405, 'nothing is accepted at the old door');
  // An old /gate.js URL is an unknown page now, so it opens the app rather than
  // the script that asked for a code.
  assert.match(String(get('/gate.js').body), /id="app"/);
  assert.doesNotMatch(String(get('/gate.js').body), /encore-gate/);
});

test('the API answers for itself, and other methods are refused', () => {
  assert.equal(webAnswer('GET', '/api/me'), null);
  assert.equal(webAnswer('POST', '/api/link/claim'), null);
  assert.equal(webAnswer('GET', '/api'), null, 'the API answers for itself');
  assert.equal(get('/', 'POST').status, 405);
});

test('the page loads only itself, Spotify and artwork', () => {
  const { headers } = get('/');
  assert.equal(headers['X-Content-Type-Options'], 'nosniff');
  assert.equal(headers['Referrer-Policy'], 'no-referrer');
  const policy = headers['Content-Security-Policy'];
  assert.match(policy, /default-src 'none'/);
  assert.match(policy, /script-src 'self'/);
  assert.match(policy, /connect-src 'self' https:\/\/accounts\.spotify\.com https:\/\/api\.spotify\.com/);
  assert.match(policy, /worker-src 'self'/);
  assert.doesNotMatch(policy, /worker-src[^;]*https?:/);
  assert.match(policy, /frame-ancestors 'none'/);
});

test('the worker is at the root, revalidates, and knows this build', () => {
  const worker = get('/sw.js');
  assert.equal(worker.status, 200);
  assert.match(worker.headers['Content-Type'], /^text\/javascript/);
  assert.equal(worker.headers['Cache-Control'], 'no-cache');
  const source = String(worker.body);
  assert.doesNotMatch(source, /__VERSION__|__SHELL__/);
  assert.match(source, /const VERSION = '[a-f0-9]{12}'/);
  assert.match(source, /\/app\.js\?v=[a-f0-9]{12}/);
  assert.match(source, /self\.addEventListener\('fetch'/);
  assert.match(source, /url\.pathname === '\/api'/, 'API calls are left alone');
  assert.match(source, /request\.headers\.has\('range'\)/, 'range requests are left alone');
  assert.match(source, /destination === 'audio'/, 'audio requests are left alone');
});

test('assets say what they are, and the page does not', () => {
  assert.match(get('/app.js').headers['Content-Type'], /^text\/javascript/);
  assert.match(get('/app.css').headers['Content-Type'], /^text\/css/);
  assert.match(get('/manifest.webmanifest').headers['Content-Type'], /^application\/manifest\+json/);
  assert.match(get('/icon-512.png').headers['Content-Type'], /^image\/png/);
  assert.match(get('/').headers['Content-Type'], /^text\/html/);
});

test('a repeat open transfers nothing, while the page revalidates', () => {
  const page = get('/');
  assert.equal(page.headers['Cache-Control'], 'no-cache', 'the page itself is always checked');
  assert.match(page.headers.ETag, /^"[a-f0-9]{12}"$/);
  assert.equal(get('/', 'GET', page.headers.ETag).status, 304);
  assert.equal(get('/', 'GET', `W/${page.headers.ETag}`).status, 304, 'a weak tag still matches');

  const src = /src="(\/app\.js\?v=[a-f0-9]{12})"/.exec(String(page.body))?.[1];
  assert.ok(src, 'the page points at a versioned script');
  const script = get(src);
  assert.equal(script.status, 200);
  assert.equal(script.headers['Cache-Control'], 'public, max-age=31536000, immutable');
  const again = get(src, 'GET', script.headers.ETag);
  assert.equal(again.status, 304, 'a second open asks and gets nothing back');
  assert.equal(again.body, '');

  // An unversioned URL is not kept for a year: it revalidates, so an old link
  // never quietly pins an old build.
  assert.equal(get('/app.js').headers['Cache-Control'], 'no-cache');
});

test('the app and its modules load as native modules, each kept for good', () => {
  const page = String(get('/').body);
  assert.match(page, /<script type="module" src="\/app\.js\?v=[a-f0-9]{12}"><\/script>/);
  const src = /src="(\/app\.js\?v=[a-f0-9]{12})"/.exec(page)?.[1];
  assert.ok(src, 'the page points at a versioned module script');
  const version = /\?v=([a-f0-9]{12})$/.exec(src)?.[1];
  assert.ok(version, 'the script carries a build version');

  // The app names its own modules at this same version, so none of them is
  // fetched at a URL that could ever hold a different build.
  const entry = String(get(src).body);
  assert.match(entry, /from '\.\/js\/boot\.js\?v=[a-f0-9]{12}'/);

  const modules = readdirSync(new URL('../assets/web/js', import.meta.url), { recursive: true, encoding: 'utf8' })
    .filter(name => name.endsWith('.js'))
    .map(name => name.replaceAll('\\', '/'));
  const worker = String(get('/sw.js').body);
  for (const name of modules) {
    const url: string = `/js/${name}?v=${version}`;
    const answer = get(url);
    assert.equal(answer.status, 200, url);
    assert.match(answer.headers['Content-Type'], /^text\/javascript/, url);
    assert.equal(answer.headers['Cache-Control'], 'public, max-age=31536000, immutable', url);
    assert.ok(worker.includes(url), `the worker precaches ${url}`);
  }
  // The same file without its version is always checked, never kept for a year.
  assert.equal(get('/js/dom.js').headers['Cache-Control'], 'no-cache');
});

test('the manifest points at versioned icons, so an update is a new URL', () => {
  const manifest = JSON.parse(String(get('/manifest.webmanifest').body)) as { icons: Array<{ src: string }> };
  for (const icon of manifest.icons) assert.match(icon.src, /\?v=[a-f0-9]{12}$/, icon.src);
  assert.equal(get('/manifest.webmanifest').headers['Cache-Control'], 'no-cache');
});

test('the manifest makes an installable app with Search and Library shortcuts', () => {
  const manifest = JSON.parse(String(get('/manifest.webmanifest').body)) as {
    id: string; scope: string; start_url: string; display: string;
    theme_color: string; background_color: string;
    icons: Array<{ sizes: string; purpose?: string }>;
    shortcuts: Array<{ url: string }>;
  };
  assert.equal(manifest.id, '/');
  assert.equal(manifest.scope, '/');
  assert.equal(manifest.start_url, '/');
  assert.equal(manifest.display, 'standalone');
  assert.equal(manifest.theme_color, '#0b0b0c');
  assert.equal(manifest.background_color, '#0b0b0c');
  assert.ok(manifest.icons.some(icon => icon.sizes === '512x512' && icon.purpose?.includes('maskable')), 'a maskable 512');
  assert.deepEqual(manifest.shortcuts.map(shortcut => shortcut.url), ['/?tab=search', '/?tab=library']);
});

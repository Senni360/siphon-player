import assert from 'node:assert/strict';
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
  assert.match(policy, /frame-ancestors 'none'/);
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

test('the manifest points at versioned icons, so an update is a new URL', () => {
  const manifest = JSON.parse(String(get('/manifest.webmanifest').body)) as { icons: Array<{ src: string }> };
  for (const icon of manifest.icons) assert.match(icon.src, /\?v=[a-f0-9]{12}$/, icon.src);
  assert.equal(get('/manifest.webmanifest').headers['Cache-Control'], 'no-cache');
});

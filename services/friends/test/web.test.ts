import assert from 'node:assert/strict';
import test from 'node:test';
import { webAnswer, type WebAnswer } from '../src/web.js';

// The phone page at encore.senni.dev. There is no door code any more: the shell
// is public, and only the account's own device secret reaches anything private.
const get = (path: string, method = 'GET'): WebAnswer => webAnswer(method, path) as WebAnswer;

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

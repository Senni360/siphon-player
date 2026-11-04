import assert from 'node:assert/strict';
import test from 'node:test';
import { sitePage } from '../src/site.js';

const GROUP = '11111111-1111-4111-8111-111111111111';
const PARTY = '22222222-2222-4222-8222-222222222222';

test('the front door says where to get Encore', () => {
  const page = sitePage('GET', '/');
  assert.equal(page?.status, 200);
  assert.match(page!.html, /Download for Windows/);
});

test('a party link opens Encore at that party', () => {
  const page = sitePage('GET', `/party/${GROUP}/${PARTY}`);
  assert.equal(page?.status, 200);
  assert.match(page!.html, new RegExp(`siphon://party/${GROUP}/${PARTY}`));
});

test('anything else in a party link goes nowhere near the page', () => {
  for (const path of ['/party/x/y', `/party/${GROUP}`, `/party/${GROUP}/"><script>alert(1)</script>`, `/party/${GROUP}/${PARTY}/more`]) {
    const page = sitePage('GET', path);
    assert.equal(page?.status, 404, path);
    assert.doesNotMatch(page!.html, /siphon:|script>alert/, path);
  }
});

test('the service keeps its own routes', () => {
  assert.equal(sitePage('GET', '/api/posts'), null);
  assert.equal(sitePage('GET', '/api'), null);
  assert.equal(sitePage('POST', '/'), null);
});

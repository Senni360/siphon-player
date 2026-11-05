import assert from 'node:assert/strict';
import test from 'node:test';
import { downloadUrl, forgetNewestRelease, sitePage } from '../src/site.js';

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
  assert.equal(sitePage('GET', '/privacy')?.status, 200);
  assert.match(sitePage('GET', '/privacy')!.html, /Your Google account/);
  assert.match(sitePage('GET', '/')!.html, /href="\/privacy"/);
  assert.equal(sitePage('GET', '/api'), null);
  assert.equal(sitePage('POST', '/'), null);
});

const ZIP_URL = 'https://github.com/Senni360/siphon-player/releases/download/v1.0.0-pre.1/siphon-v1.0.0-pre.1-x86_64-pc-windows-msvc.zip';
const github = (releases: unknown, ok = true) => (async () => new Response(JSON.stringify(releases), { status: ok ? 200 : 403 })) as unknown as typeof fetch;

test('the button downloads the newest zip, previews included, and asks GitHub rarely', async () => {
  forgetNewestRelease();
  let asked = 0;
  const old = 'https://github.com/Senni360/siphon-player/releases/download/v1.0.0-pre.9/siphon-v1.0.0-pre.9-x86_64-pc-windows-msvc.zip';
  const ask = (async () => { asked++; return new Response(JSON.stringify([
    { draft: true, published_at: '2030-01-01T00:00:00Z', assets: [{ name: 'siphon-v9-x86_64-pc-windows-msvc.zip', browser_download_url: 'https://github.com/Senni360/siphon-player/draft.zip' }] },
    // GitHub really does list pre.9 above pre.10.
    { published_at: '2026-01-01T00:00:00Z', assets: [{ name: 'siphon-v1.0.0-pre.9-x86_64-pc-windows-msvc.zip', browser_download_url: old }] },
    { published_at: '2026-02-01T00:00:00Z', assets: [{ name: 'checksums.txt' }, { name: 'siphon-v1.0.0-pre.1-x86_64-pc-windows-msvc.zip', browser_download_url: ZIP_URL }] },
  ])); }) as unknown as typeof fetch;
  assert.equal(await downloadUrl(ask, 1000), ZIP_URL);
  assert.equal(await downloadUrl(ask, 2000), ZIP_URL);
  assert.equal(asked, 1);
});

test('when GitHub cannot say, the releases page still works', async () => {
  const page = 'https://github.com/Senni360/siphon-player/releases';
  forgetNewestRelease();
  assert.equal(await downloadUrl(github([], false), 1000), page);
  assert.equal(await downloadUrl((async () => { throw new Error('down'); }) as unknown as typeof fetch, 1000), page);
  assert.equal(await downloadUrl(github([{ published_at: '2026-01-01T00:00:00Z', assets: [{ name: 'siphon-v1-x86_64-pc-windows-msvc.zip', browser_download_url: 'https://evil.example/a.zip' }] }]), 1000), page);
});

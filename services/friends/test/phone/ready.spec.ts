import { test, expect } from './fixtures.js';

test('with the computer off, a list says which songs still need it, and stops once it is back', async ({ phone, page, server }) => {
  server.enableEdge();
  server.setComputerOffline(true);
  const spotify = (await import('./server.js')).SONGS.slice(1, 60).filter(song => song.uri.startsWith('spotify:'));
  // The edge has copies of all but two of the mix's Spotify songs.
  for (const song of spotify.slice(2)) server.edgeSaved[song.uri] = { sourceUri: 'soundcloud:encore/song-1' };
  await page.goto(`${server.url}/`);
  await expect(page.locator('nav.tabs')).toBeVisible();
  await phone.goTab('library');
  await page.locator('main .row.place').filter({ hasText: 'Everything, 200 Songs' }).locator('button.words').click();
  await expect(page.locator('main .ready-note')).toHaveText('2 songs here play only while your computer is on. Encore makes them ready as soon as it is.');
  const marked = page.locator('main .row.waiting');
  await expect(marked).toHaveCount(2);
  await expect(marked.first().locator('.by')).toContainText('Needs your computer');
  await expect(marked.first()).toHaveAttribute('data-uri', spotify[0]!.uri);

  server.setComputerOffline(false);
  await expect(marked).toHaveCount(0, { timeout: 20_000 });
  await expect(page.locator('main .ready-note')).toBeHidden();
});

test('with the computer on, nothing is marked', async ({ phone, page, server }) => {
  server.enableEdge();
  await phone.open();
  await phone.openMix();
  await expect(page.locator('main .row.waiting')).toHaveCount(0);
  await expect(page.locator('main .ready-note')).toBeHidden();
});

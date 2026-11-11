import { test, expect, type Phone } from './fixtures.js';

/** Opens the app without assuming a computer: with it off, the edge answers. */
async function openAlone(phone: Phone): Promise<void> {
  await phone.page.goto(`${phone.server.url}/`);
  await expect(phone.page.locator('nav.tabs')).toBeVisible();
}

const streamOf = async (phone: Phone): Promise<string> => (await phone.audio())?.src ?? '';

test('with the computer off, the edge fills home, the library, a playlist, search and an artist', async ({ phone, server }) => {
  server.enableEdge();
  server.setComputerOffline(true);
  await openAlone(phone);

  // Home lands in about a second, not after the wait for a computer that is off.
  await expect(phone.page.locator('main .lead .name')).toHaveText(server.songAt(0).name);
  await expect(phone.page.getByText("Encore isn't open on your computer")).toHaveCount(0);
  // Everything works from the server, so no banner says the computer is away.
  await expect(phone.page.locator('.banner')).toHaveCount(0);

  // The library: Spotify playlists, and the mixed ones the edge knows beside them.
  await phone.goTab('library');
  const deepFocus = phone.page.locator('main .row.place').filter({ hasText: 'Deep Focus' });
  await expect(deepFocus).toHaveCount(1);
  await expect(deepFocus).toContainText('60 songs');
  await expect(phone.page.locator('main .row.place').filter({ hasText: 'Everything, 200 Songs' })).toHaveCount(1);

  // A playlist opens whole, page after page.
  await deepFocus.locator('button.words').click();
  await expect(phone.page.locator('h1.title')).toHaveText('Deep Focus');
  await expect(phone.page.locator('main .list-head .note')).toContainText('60 songs');
  await expect(phone.page.locator('main .row')).toHaveCount(50);
  await phone.page.locator('main .rows-end').scrollIntoViewIfNeeded();
  await expect(phone.page.locator('main .row')).toHaveCount(60, { timeout: 15_000 });

  // Search: rows from both sources the edge knows.
  await phone.goTab('search');
  await phone.page.locator('main input[type="search"]').fill('Neon');
  await expect(phone.page.locator('main .results')).toContainText('Midnight City', { timeout: 15_000 });
  await expect(phone.page.locator('main .results')).toContainText('Golden Hour');

  // An artist from those results is a page of its own.
  await phone.page.locator('main .shelf .card').first().click();
  await expect(phone.page.locator('h1.title')).toHaveText('Neon Harbour');
  await expect(phone.page.locator('main .row').first()).toBeVisible();
});

test('a mixed playlist\'s local file plays from the edge', async ({ phone, server }) => {
  server.enableEdge();
  server.setComputerOffline(true);
  await openAlone(phone);

  await phone.goTab('library');
  await phone.page.locator('main .row.place').filter({ hasText: 'Everything, 200 Songs' }).locator('button.words').click();
  await expect(phone.page.locator('h1.title')).toHaveText('Everything, 200 Songs');

  const row = phone.row(0);
  await expect(row.locator('.name')).toHaveText(server.songAt(0).name);
  await row.locator('button.words').click();
  await expect(phone.page.locator('.mini .name')).toHaveText(server.songAt(0).name);
  await expect.poll(() => streamOf(phone), { timeout: 20_000 }).toContain('/v1/media/');
  await expect.poll(() => server.countRequests('POST /v1/media-token'), { timeout: 20_000 }).toBeGreaterThan(0);
  // This song's own address did not come from the computer.
  expect(server.sent.filter(command => command.kind === 'stream' && command.uri === 'spotify:local:Sam:Kitchen:Midnight%20City:214')).toEqual([]);
});

test('an edge without a vault falls back exactly as before', async ({ phone, server, observed }) => {
  observed.allowConsole(/Failed to load resource: the server responded with a status of 503/);
  server.enableEdge();
  server.setEdgeNoVault(true);
  server.setComputerOffline(true);
  await openAlone(phone);

  // The edge was asked, refused, and the page went on to the computer just as it does today.
  await expect.poll(() => server.countRequests('GET /v1/home'), { timeout: 15_000 }).toBeGreaterThan(0);
  await expect(phone.page.getByText('Made for you')).toBeVisible();
  await expect(phone.page.locator('main .lead')).toBeVisible();
  await expect.poll(() => server.sent.filter(command => command.kind === 'home').length, { timeout: 15_000 }).toBeGreaterThan(0);
  await expect(phone.page.locator('main .empty')).toHaveCount(0);
});

test('with the computer on, the edge library is never asked', async ({ phone, server }) => {
  server.enableEdge();
  await phone.open();

  await expect(phone.page.getByText('Made for you')).toBeVisible();
  await phone.goTab('library');
  await expect(phone.page.locator('main .row.place').filter({ hasText: 'Deep Focus' })).toHaveCount(1);
  await phone.goTab('search');
  await phone.page.locator('main input[type="search"]').fill('Neon');
  await expect(phone.page.locator('main .results')).toContainText('Midnight City');

  const asked = server.requests.filter(request => / \/v1\/(home|search|library|artist|album)/.test(request));
  expect(asked).toEqual([]);
});

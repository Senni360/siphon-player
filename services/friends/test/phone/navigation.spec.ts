import { test, expect } from './fixtures.js';

test('the back gesture closes the now-playing sheet before leaving the playlist', async ({ phone }) => {
  await phone.open();
  await phone.goTab('library');
  await expect(phone.page.locator('h1.title')).toHaveText('Library');

  await phone.openMix();
  const title = phone.page.locator('h1.title');
  await expect(title).toHaveText('Everything, 200 Songs');

  await phone.playRow(0);
  await phone.openSheet();

  // First back: the sheet goes, the playlist stays.
  await phone.page.evaluate(() => history.back());
  await expect(phone.page.locator('.sheet')).toHaveCount(0);
  await expect(title).toHaveText('Everything, 200 Songs');

  // Second back: the playlist goes, the library stays.
  await phone.page.evaluate(() => history.back());
  await expect(title).toHaveText('Library');
  await expect(phone.page.locator('main .row').first()).toContainText('Liked Songs');
});

test('only Spotify sign-in is allowed to leave the app', async ({ phone, server }) => {
  await phone.open();
  await phone.goTab('library');

  // The one journey out of the origin a test may take: the listener's own
  // Spotify app, at the address the page hands them.
  phone.guard.allowNext(/^https:\/\/accounts\.spotify\.com\/authorize\?/);
  await phone.page.getByRole('button', { name: 'Keep Spotify going with the computer off' }).click();
  await phone.page.getByLabel('Spotify Client ID').fill('a1b2c3d4e5f6g7h8');
  await phone.page.getByRole('button', { name: 'Continue to Spotify' }).click();

  await expect(phone.page).toHaveURL(/^https:\/\/accounts\.spotify\.com\/authorize\?/);
  expect(phone.guard.allowedUrls).toHaveLength(1);

  const address = new URL(phone.guard.allowedUrls[0]!);
  expect(address.origin).toBe('https://accounts.spotify.com');
  expect(address.pathname).toBe('/authorize');
  expect(address.searchParams.get('client_id')).toBe('a1b2c3d4e5f6g7h8');
  expect(address.searchParams.get('response_type')).toBe('code');
  expect(address.searchParams.get('code_challenge_method')).toBe('S256');
  expect(address.searchParams.get('code_challenge')?.length).toBeGreaterThan(20);
  expect(address.searchParams.get('redirect_uri')).toBe(`${server.url}/callback`);
  expect(address.searchParams.get('scope')).toContain('user-read-playback-state');
});

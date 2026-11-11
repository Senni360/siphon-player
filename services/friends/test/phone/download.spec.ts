import { test, expect } from './fixtures.js';

test('a song is sent to the computer to download, even while the phone plays', async ({ phone, page, server }) => {
  await phone.open();
  await phone.openMix();
  await phone.playRow(1);
  const row = phone.row(4);
  const uri = await row.getAttribute('data-uri');
  const name = (await row.locator('.name').innerText()).trim();

  await row.locator('button.more').click();
  await page.locator('.actions .choice', { hasText: 'Download on your computer' }).click();
  await expect(page.locator('.toast')).toHaveText('Downloading on your computer');
  await expect.poll(() => server.downloads.length).toBe(1);
  expect(server.downloads[0]).toMatchObject({ uri, name });
  expect(Array.isArray(server.downloads[0]!.artists)).toBe(true);
});

test('with the computer off there is nothing to download on', async ({ phone, page, server }) => {
  await phone.open();
  await phone.openMix();
  server.setComputerOffline(true);
  await expect(page.locator('.banner')).toBeVisible({ timeout: 40_000 });
  await phone.row(4).locator('button.more').click();
  await expect(page.locator('.actions .choice', { hasText: 'Play next' })).toBeVisible();
  await expect(page.locator('.actions .choice', { hasText: 'Download on your computer' })).toHaveCount(0);
});

test('a computer that has not learned to download is not asked', async ({ phone, page, server }) => {
  server.setComputerState({ can: undefined });
  await phone.open();
  await phone.openMix();
  await phone.row(4).locator('button.more').click();
  await expect(page.locator('.actions .choice', { hasText: 'Play next' })).toBeVisible();
  await expect(page.locator('.actions .choice', { hasText: 'Download on your computer' })).toHaveCount(0);
});

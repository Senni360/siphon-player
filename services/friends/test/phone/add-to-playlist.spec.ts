import { test, expect } from './fixtures.js';

test('a song is added to a playlist from its menu, and that playlist comes first next time', async ({ phone, page, server }) => {
  await phone.open();
  await phone.openMix();
  const row = phone.row(4);
  const uri = await row.getAttribute('data-uri');

  await row.locator('button.more').click();
  await page.locator('.actions .choice', { hasText: 'Add to playlist' }).click();
  const picker = page.getByRole('dialog', { name: 'Add to playlist' });
  await expect(picker.locator('.choice')).toHaveCount(4);
  await picker.locator('.choice', { hasText: 'Sunday Kitchen' }).click();
  await expect(page.locator('.toast')).toHaveText('Added to Sunday Kitchen');
  await expect.poll(() => server.added.length).toBe(1);
  expect(server.added[0]).toMatchObject({ target: 'spotifast:playlist:mix-sunday', uri });

  await phone.row(5).locator('button.more').click();
  await page.locator('.actions .choice', { hasText: 'Add to playlist' }).click();
  await expect(page.getByRole('dialog', { name: 'Add to playlist' }).locator('.choice').first()).toContainText('Sunday Kitchen');
});

test('a computer that cannot add offers no Add to playlist', async ({ phone, page, server }) => {
  server.setComputerState({ can: ['download'] });
  await phone.open();
  await phone.openMix();
  await phone.row(4).locator('button.more').click();
  await expect(page.locator('.actions .choice', { hasText: 'Play next' })).toBeVisible();
  await expect(page.locator('.actions .choice', { hasText: 'Add to playlist' })).toHaveCount(0);
});

import { test, expect } from './fixtures.js';

test('a long playlist narrows to what is typed, and a found song plays in its place', async ({ phone, page }) => {
  await phone.open();
  await phone.openMix();
  const after = (await phone.row(4).locator('.name').innerText()).trim();
  const wanted = (await phone.row(3).locator('.name').innerText()).trim();

  const find = page.getByRole('searchbox', { name: 'Find in this list' });
  await find.fill(wanted.toUpperCase());
  const found = page.locator('main .row');
  await expect(found.first().locator('.name')).toHaveText(wanted);
  expect(await found.count()).toBeLessThan(50);

  // Played from what was found, it carries on with the playlist, not with the matches.
  await found.first().locator('button.words').click();
  await expect(page.locator('.mini .name')).toHaveText(wanted);
  await page.locator('.mini button.skip').click();
  await expect(page.locator('.mini .name')).toHaveText(after);

  await find.fill('no such song anywhere');
  await expect(page.locator('main .none-found')).toContainText('Nothing here matches');
  await page.getByRole('button', { name: 'Clear' }).click();
  await expect(found).toHaveCount(50);
});

test('a short list has no field to find in', async ({ phone, page }) => {
  await phone.open();
  await phone.goTab('library');
  await expect(page.locator('main .row').first()).toBeVisible();
  await expect(page.getByRole('searchbox')).toHaveCount(0);
});

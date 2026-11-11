import { test, expect } from './fixtures.js';

test('new releases say what they are, an album opens and a single plays', async ({ phone, page }) => {
  await phone.open();
  const shelf = page.locator('main .release');
  await expect(shelf).toHaveCount(3);
  await expect(shelf.nth(0).locator('.by')).toHaveText('Album · Bonobo');
  await expect(shelf.nth(1).locator('.by')).toHaveText('EP · Tycho');
  await expect(shelf.nth(2).locator('.by')).toHaveText(/^Single · /);
  // An album and an EP carry their record; a single is the cover alone.
  await expect(shelf.nth(0).locator('.record')).toHaveCount(1);
  await expect(shelf.nth(2).locator('.record')).toHaveCount(0);

  const single = (await shelf.nth(2).locator('.name').innerText()).trim();
  await shelf.nth(2).click();
  await expect(page.locator('.mini .name')).toHaveText(single);

  await shelf.nth(0).click();
  await expect(page.locator('main .list-head')).toBeVisible();
});

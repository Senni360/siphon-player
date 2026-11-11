import { test, expect } from './fixtures.js';

test("a liked song's menu offers to remove it from your likes", async ({ phone, page }) => {
  await phone.open();
  await phone.goTab('library');
  await page.locator('main .row.place').filter({ hasText: 'Liked Songs' }).locator('button.words').click();
  await phone.row(0).locator('button.more').click();
  const like = page.locator('.actions .choice.liked');
  await expect(like).toHaveText('Remove from your likes');
  await like.click();
  await expect(page.locator('.toast')).toHaveText('Removed from your likes');
});

test("a song that isn't liked offers Like", async ({ phone, page }) => {
  await phone.open();
  await phone.openMix();
  await phone.row(30).locator('button.more').click();
  await expect(page.locator('.actions .choice', { hasText: 'Like' }).first()).toBeVisible();
  await expect(page.locator('.actions .choice.liked')).toHaveCount(0);
});

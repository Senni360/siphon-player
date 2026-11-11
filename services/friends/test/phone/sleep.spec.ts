import { test, expect } from './fixtures.js';

const openTimer = async (page: import('@playwright/test').Page) => {
  await page.locator('.sheet button[aria-label="More for this song"]').click();
  await page.locator('.actions .choice', { hasText: 'Sleep timer' }).click();
  await expect(page.getByRole('dialog', { name: 'Sleep timer' })).toBeVisible();
};

test('the sleep timer pauses the phone when its minutes are up', async ({ phone, page }) => {
  await phone.open();
  await phone.openMix();
  const name = await phone.playRow(3);
  await phone.openSheet();
  await openTimer(page);
  await page.locator('.actions .choice', { hasText: '15 minutes' }).click();
  await expect(page.locator('.toast')).toHaveText('Stops in 15 minutes');

  // Still playing a minute before; paused once the time has passed, on the same song.
  await page.evaluate(() => { const real = Date.now; Date.now = () => real() + 14 * 60000; });
  await phone.advance(1);
  expect((await phone.audio())?.paused).toBe(false);
  await page.evaluate(() => { const shifted = Date.now; Date.now = () => shifted() + 2 * 60000; });
  await phone.advance(1);
  await expect.poll(async () => (await phone.audio())?.paused).toBe(true);
  await expect(page.locator('.sheet .big')).toHaveText(name);

  // It is spent: pressing play carries on, and nothing pauses it again.
  await page.locator('.sheet .main').click();
  await phone.advance(1);
  expect((await phone.audio())?.paused).toBe(false);
});

test('set to the end of the song, the next one does not start', async ({ phone, page }) => {
  await phone.open();
  await phone.openMix();
  const name = await phone.playRow(3);
  await phone.openSheet();
  await openTimer(page);
  await page.locator('.actions .choice', { hasText: 'End of this song' }).click();
  await expect(page.locator('.toast')).toHaveText('Stops after this song');

  // The menu says what it is set to, and offers to turn it off.
  await page.locator('.sheet button[aria-label="More for this song"]').click();
  await expect(page.locator('.actions .choice', { hasText: 'Sleep timer' })).toContainText('Stops after this song');
  await page.locator('.scrim').click({ position: { x: 20, y: 20 } });

  await phone.advance(100000);
  await expect.poll(async () => (await phone.audio())?.ended).toBe(true);
  await expect(page.locator('.sheet .big')).toHaveText(name);
  await expect(page.locator('.sheet .main')).toHaveAttribute('aria-label', 'Play');
});

test("the computer's own playback has no sleep timer here", async ({ phone, page }) => {
  await page.addInitScript(() => { localStorage.setItem('encore.target', 'computer'); });
  await phone.open();
  await phone.openSheet();
  await page.locator('.sheet button[aria-label="More for this song"]').click();
  await expect(page.locator('.actions .choice', { hasText: 'Play next' })).toBeVisible();
  await expect(page.locator('.actions .choice', { hasText: 'Sleep timer' })).toHaveCount(0);
});

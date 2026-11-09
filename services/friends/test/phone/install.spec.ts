import { test, expect } from './fixtures.js';
import type { Page } from '@playwright/test';

/** Home with the stored sign-in, and the install note allowed to appear once. */
async function home(page: Page, server: { url: string }, installKey: string): Promise<void> {
  await page.addInitScript(key => {
    if (sessionStorage.getItem('encore.install-probe')) return;
    sessionStorage.setItem('encore.install-probe', '1');
    localStorage.removeItem(key);
  }, installKey);
  await page.goto(`${server.url}/`);
  await expect(page.locator('nav.tabs')).toBeVisible();
}

test('an iPhone is shown how to install once, and can put the note away for good',
  async ({ page, server, browserName }) => {
    test.skip(browserName !== 'webkit', 'the iPhone home-screen route');
    await home(page, server, 'encore.install');

    const tip = page.locator('.banner.install');
    await expect(tip).toBeVisible();
    await expect(tip).toContainText('Add to Home Screen');

    await tip.getByRole('button', { name: 'Dismiss' }).click();
    await expect(tip).toHaveCount(0);
    expect(await page.evaluate(() => localStorage.getItem('encore.install'))).toBe('1');

    await page.reload();
    await expect(page.locator('nav.tabs')).toBeVisible();
    await expect(page.locator('.banner.install')).toHaveCount(0, { timeout: 2_000 });
  });

test('Android installs through the browser prompt when it offers one',
  async ({ page, server, browserName }) => {
    test.skip(browserName !== 'chromium', 'beforeinstallprompt is Chromium only');
    await home(page, server, 'encore.install');
    await expect(page.locator('.banner.install')).toHaveCount(0);

    await page.evaluate(() => {
      const event = new Event('beforeinstallprompt', { cancelable: true });
      Object.assign(event, {
        prompt: () => { (window as unknown as { __prompted?: boolean }).__prompted = true; },
        userChoice: Promise.resolve({ outcome: 'accepted', platform: 'web' }),
      });
      window.dispatchEvent(event);
    });

    const tip = page.locator('.banner.install');
    await expect(tip).toBeVisible();
    await tip.getByRole('button', { name: 'Install', exact: true }).click();
    await expect.poll(() => page.evaluate(() => (window as unknown as { __prompted?: boolean }).__prompted)).toBe(true);
    await expect(tip).toHaveCount(0);
    expect(await page.evaluate(() => localStorage.getItem('encore.install'))).toBe('installed');
  });

test('a manifest shortcut opens its tab and leaves a clean address', async ({ page, server }) => {
  await page.goto(`${server.url}/?tab=library`);
  await expect(page.locator('nav.tabs')).toBeVisible();
  await expect(page.locator('h1.title')).toHaveText('Library');

  const address = new URL(page.url());
  expect(address.pathname).toBe('/');
  expect(address.search).toBe('');
});

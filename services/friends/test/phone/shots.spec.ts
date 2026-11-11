// Pictures of every screen, for looking at the design. Not part of the normal run:
//   ENCORE_SHOTS=out-dir npx playwright test --config test/phone/playwright.config.ts shots
import { test, expect } from './fixtures.js';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';

const OUT = process.env.ENCORE_SHOTS;
test.skip(!OUT, 'set ENCORE_SHOTS to a folder to take the pictures');
test.describe.configure({ mode: 'serial' });

/** A cover with some colour in it, different for every address. */
function cover(url: string): string {
  let seed = 0;
  for (const letter of url) seed = (seed * 31 + letter.charCodeAt(0)) >>> 0;
  const hue = seed % 360;
  const other = (hue + 40 + (seed >> 8) % 80) % 360;
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 300 300"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
    <stop offset="0" stop-color="hsl(${hue} 62% 42%)"/><stop offset="1" stop-color="hsl(${other} 58% 18%)"/></linearGradient></defs>
    <rect width="300" height="300" fill="url(#g)"/><circle cx="${180 + (seed % 60)}" cy="${80 + (seed >> 4) % 60}" r="120" fill="hsl(${other} 70% 55%)" opacity=".35"/>
    <circle cx="70" cy="250" r="130" fill="hsl(${hue} 60% 30%)" opacity=".55"/></svg>`;
}

test('every screen', async ({ phone, page, server, observed }, info) => {
  test.setTimeout(180_000);
  // Taking a picture adds a stylesheet of Playwright's own, which the page's policy refuses.
  observed.allowConsole(/Refused to apply a stylesheet/);
  const dir = join(OUT!, info.project.name.replaceAll(' ', '-'));
  mkdirSync(dir, { recursive: true });
  let step = 0;
  const shot = async (name: string) => {
    // A picture of what has arrived, not of the bones standing in for it.
    await expect(page.locator('main [aria-busy="true"]')).toHaveCount(0, { timeout: 15_000 });
    await page.waitForTimeout(650);
    await page.screenshot({ path: join(dir, `${String(++step).padStart(2, '0')}-${name}.png`) });
  };
  await page.route(url => !url.href.startsWith(server.url), async route => {
    if (route.request().resourceType() !== 'image') return route.fallback();
    await route.fulfill({ status: 200, contentType: 'image/svg+xml', body: cover(route.request().url()) });
  });

  await phone.open();
  await shot('home');
  await page.locator('main').evaluate(node => node.scrollTo(0, 700));
  await shot('home-lower');

  await phone.goTab('search');
  await shot('search-empty');
  const input = page.getByRole('searchbox', { name: 'Search' });
  await input.fill('neon harbour');
  await input.press('Enter');
  await expect(page.locator('main .row').first()).toBeVisible();
  await shot('search-results');

  await phone.goTab('library');
  await shot('library');
  await phone.openMix();
  await shot('list');
  await page.locator('main').evaluate(node => node.scrollTo(0, 520));
  await shot('list-scrolled');
  await phone.playRow(9);
  await shot('list-playing');
  await page.locator('main .row').nth(11).locator('button.more').click();
  await shot('song-actions');
  await page.locator('.scrim').click({ position: { x: 20, y: 20 } });

  await phone.openSheet();
  await phone.advance(48);
  await shot('now-playing');
  await page.locator('.sheet .tools button', { hasText: 'Queue' }).click();
  await shot('queue');
  await page.locator('.sheet .tools button', { hasText: 'Lyrics' }).click();
  await shot('lyrics');
  await page.locator('.sheet .tools button', { hasText: 'Lyrics' }).click();
  await page.locator('.sheet .where').click();
  await shot('devices');
  await page.locator('.scrim').click({ position: { x: 20, y: 20 } });
  await page.locator('.sheet .main').click();
  await shot('now-playing-paused');
  await page.locator('.sheet button[aria-label="Close"]').click();
  await expect(page.locator('.sheet')).toHaveCount(0);

  await phone.goTab('group');
  await shot('group');
  await phone.goTab('library');
  await page.getByRole('button', { name: 'Settings' }).click();
  await shot('settings');
  await page.evaluate(() => history.back());

  await phone.goTab('search');
  await page.getByRole('searchbox', { name: 'Search' }).fill('neon');
  await page.locator('.card.circle').first().click();
  await expect(page.locator('.hero')).toBeVisible();
  await shot('artist');

  server.setComputerOffline(true);
  await phone.goTab('home');
  await expect(page.locator('.banner')).toBeVisible({ timeout: 40_000 });
  await shot('computer-off');
});

test('the door', async ({ page, server, observed }, info) => {
  observed.allowConsole(/Refused to apply a stylesheet/);
  const dir = join(OUT!, info.project.name.replaceAll(' ', '-'));
  mkdirSync(dir, { recursive: true });
  await page.addInitScript(() => { localStorage.clear(); });
  await page.goto(`${server.url}/`);
  await expect(page.locator('form.door')).toBeVisible();
  await page.waitForTimeout(600);
  await page.screenshot({ path: join(dir, '00-link.png') });
});

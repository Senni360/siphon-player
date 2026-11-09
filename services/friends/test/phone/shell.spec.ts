import { test, expect } from './fixtures.js';
import type { Page } from '@playwright/test';

test.use({ serviceWorkers: 'allow' });

/** A fresh phone: no stored sign-in, so this is the public link screen. */
async function fresh(page: Page, server: { url: string }): Promise<void> {
  await page.addInitScript(() => { localStorage.clear(); });
  await page.goto(`${server.url}/`);
  await expect(page.locator('form.door')).toBeVisible();
}

/** Waits until this page is controlled, so a reload really goes through the worker. */
async function controlled(page: Page): Promise<void> {
  await page.evaluate(async () => {
    if (!('serviceWorker' in navigator)) throw new Error('no service worker here');
    await navigator.serviceWorker.ready;
    if (!navigator.serviceWorker.controller) {
      await new Promise<void>(resolve => {
        navigator.serviceWorker.addEventListener('controllerchange', () => resolve(), { once: true });
      });
    }
  });
}

test('the worker leaves the API and range requests on the network', async ({ page, server }) => {
  await fresh(page, server);
  await controlled(page);

  // An ordinary API call is still the service's, not a cached answer.
  const before = server.countRequests('GET /api/me');
  const status = await page.evaluate(async () =>
    (await fetch('/api/me', { headers: { authorization: `Bearer ${'a'.repeat(64)}` } })).status);
  expect(status).toBe(200);
  expect(server.countRequests('GET /api/me')).toBe(before + 1);

  // A range request for a shell path has the shape of an audio request, and is
  // left to the network too.
  const rangedBefore = server.countRequests('GET /app.js');
  const ranged = await page.evaluate(async () =>
    (await fetch('/app.js?probe=range', { headers: { range: 'bytes=0-15' }, cache: 'no-store' })).status);
  expect(ranged).toBe(200);
  expect(server.countRequests('GET /app.js')).toBe(rangedBefore + 1);
});

test('an offline open still finds the shell', async ({ page, context, observed, server }) => {
  await fresh(page, server);
  await controlled(page);

  // Nothing local answers from here on: only the worker's cache can.
  observed.allowConsole(/Failed to load resource|ERR_INTERNET_DISCONNECTED|Load failed|could not be loaded/);
  await context.unrouteAll();
  await server.close();
  await page.reload();

  await expect(page.locator('form.door')).toBeVisible({ timeout: 20_000 });
  await expect(page.locator('link[rel="manifest"]')).toHaveCount(1);
});

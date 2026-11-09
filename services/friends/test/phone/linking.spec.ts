import { test, expect } from './fixtures.js';

test('a /link address claims its code without typing, then clears the fragment', async ({ page, server }) => {
  // A fresh phone: the fixture's stored sign-in is removed before the page runs.
  await page.addInitScript(() => { localStorage.clear(); });
  await page.goto(`${server.url}/link#abcd-1234`);

  await expect(page.locator('nav.tabs')).toBeVisible({ timeout: 20_000 });
  expect(server.claims).toHaveLength(1);
  expect(server.claims[0]).toMatchObject({ code: 'ABCD1234' });

  const address = new URL(page.url());
  expect(address.pathname).toBe('/');
  expect(address.hash).toBe('');
  await expect(page.locator('form.door')).toHaveCount(0);
});

test('typing the code still links, for a link that was read out loud', async ({ page, server }) => {
  await page.addInitScript(() => { localStorage.clear(); });
  await page.goto(`${server.url}/link`);

  await expect(page.locator('form.door')).toBeVisible();
  await page.locator('form.door input.code').fill('abcd1234');
  await page.locator('form.door button.primary').click();

  await expect(page.locator('nav.tabs')).toBeVisible({ timeout: 20_000 });
  expect(server.claims).toHaveLength(1);
  expect(server.claims[0]).toMatchObject({ code: 'abcd1234' });
});

test('a phone removed from the account says so, and offers a way back', async ({ page, phone, server, observed }) => {
  await phone.open();
  server.revoke();
  // The page's own 401s are the point of this test; Chromium logs each one, and
  // WebKit reports the in-flight reply fetch that the removal interrupts.
  observed.allowConsole(/401 \(Unauthorized\)/);
  observed.allowPageError(/access control checks/);
  await page.reload();

  await expect(page.locator('.door h1')).toHaveText('This phone was signed out', { timeout: 20_000 });
  await expect(page.locator('.door')).toContainText('Link a phone');
  await expect(page.locator('form.door')).toHaveCount(0);

  await page.getByRole('button', { name: 'Link this phone again' }).click();
  await expect(page.locator('form.door input.code')).toBeVisible();
});

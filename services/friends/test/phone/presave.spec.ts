import { test, expect } from './fixtures.js';

test("an artist's upcoming album is pre-saved from the phone, at once", async ({ phone, page, server }) => {
  await phone.open();
  await phone.goTab('search');
  await page.getByRole('searchbox', { name: 'Search' }).fill('neon');
  await page.locator('.card.circle').first().click();
  const card = page.locator('main .upcoming');
  await expect(card).toContainText('Afterglow');
  await expect(card.locator('small')).toHaveText(/^Out in 9 days · /);
  const button = card.locator('button.presave');
  await expect(button).toHaveText('Pre-save');
  await button.click();
  await expect(button).toHaveText('Pre-saved');
  await expect.poll(() => server.presaves.length).toBe(1);
  expect(server.presaves[0]).toMatchObject({ uri: 'spotify:album:5BYKG4MHfySZthe2W6r7n4', saved: true });
  await button.click();
  await expect(button).toHaveText('Pre-save');
  await expect.poll(() => server.presaves.length).toBe(2);
});

test('a computer that cannot pre-save offers no button', async ({ phone, page, server }) => {
  server.setComputerState({ can: ['download'] });
  await phone.open();
  await phone.goTab('search');
  await page.getByRole('searchbox', { name: 'Search' }).fill('neon');
  await page.locator('.card.circle').first().click();
  await expect(page.locator('main .upcoming')).toContainText('Afterglow');
  await expect(page.locator('main .upcoming button.presave')).toHaveCount(0);
});

import { test, expect } from './fixtures.js';

test('opens signed in, connected, with no link screen', async ({ phone, server }) => {
  await phone.open();

  await expect(phone.page.locator('h1.title')).toHaveText(/^Good (night|morning|afternoon|evening)$/);
  await expect(phone.page.locator('nav.tabs button')).toHaveCount(4);
  await expect(phone.page.locator('form.door')).toHaveCount(0);

  // The computer's state has landed, so the mini player shows what it plays.
  await expect(phone.page.locator('.mini')).toBeVisible();
  expect(server.state.song).not.toBeNull();
  await expect(phone.page.locator('.mini .name')).toHaveText(server.state.song!.name);
  await expect(phone.page.locator('.mini .by')).toHaveText(server.state.song!.artists.join(', '));

  // Talked to the service with the stored secret, over the event stream too.
  expect(server.countRequests('GET /api/live/remote/state')).toBeGreaterThan(0);
  expect(server.countRequests('GET /api/live/remote/events')).toBeGreaterThan(0);
  expect(server.eventConnections).toBeGreaterThan(0);
});

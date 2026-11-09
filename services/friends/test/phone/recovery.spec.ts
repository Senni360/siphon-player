import { test, expect } from './fixtures.js';

test('a dropped event stream comes back and keeps the app in step', async ({ phone, server }) => {
  await phone.open();

  const before = server.eventRequests;
  server.dropEventStream(1);
  server.endEventStreams();

  // The clean close is retried at once, the dropped one right after it.
  await expect.poll(() => server.eventRequests, { timeout: 20_000 }).toBeGreaterThanOrEqual(before + 2);
  expect(server.droppedStreams).toBe(1);

  // A change on the computer still lands, over the stream that came back.
  const next = server.songAt(121);
  server.setComputerState({ song: next, playing: false, position_ms: 0, next: [] });
  await expect(phone.page.locator('.mini .name')).toHaveText(next.name, { timeout: 15_000 });
});

test('going offline and coming back keeps the phone playing', async ({ phone, server, context, observed }) => {
  await phone.open();
  // WebKit reports the requests cut off by going offline; that is the failure under test.
  observed.allowConsole(/^Failed to load resource: WebKit encountered an internal error/);
  await phone.openMix();
  const name = await phone.playRow(0);
  await phone.advance(3);

  const stateBefore = server.countRequests('GET /api/live/remote/state');
  await context.setOffline(true);

  // The song on this phone doesn't need the network, so it plays on.
  await phone.advance(4);
  expect((await phone.audio())!.paused).toBe(false);
  expect((await phone.audio())!.currentTime).toBeGreaterThanOrEqual(7);
  await expect(phone.page.locator('.mini .name')).toHaveText(name);

  await context.setOffline(false);

  // The page finds the service again, and the song is still the same one.
  await expect.poll(() => server.countRequests('GET /api/live/remote/state'), { timeout: 25_000 })
    .toBeGreaterThan(stateBefore);
  await phone.advance(2);
  expect((await phone.audio())!.paused).toBe(false);
  await expect(phone.page.locator('.mini .name')).toHaveText(name);
  expect(new URL(phone.page.url()).origin).toBe(server.url);
});

test('a failed search leaves the page usable', async ({ phone, server, observed }) => {
  await phone.open();
  // The 500 is the failure under test; the browser's own note about it is expected.
  observed.allowConsole(/^Failed to load resource: the server responded with a status of 500 /);
  server.failNext('search', { status: 500, error: 'Search is down.' });

  await phone.goTab('search');
  const input = phone.page.getByRole('searchbox', { name: 'Search' });
  await input.fill('neon harbour');
  await input.press('Enter');

  const failure = phone.page.locator('main .empty');
  await expect(failure).toContainText('Search is down.');
  await expect(failure.getByRole('button', { name: 'Try again' })).toBeVisible();

  // The rest of the app still works.
  await phone.goTab('library');
  await expect(phone.page.locator('h1.title')).toHaveText('Library');
  await expect(phone.page.locator('main .row').first()).toContainText('Liked Songs');
});

// fixme: a media error still skips to the next song instead of holding the song where it is.
test.fixme('a mid-song media error stays on the song and the page', async ({ phone }) => {
  await phone.open();
  await phone.openMix();

  // Row 1 is a SoundCloud song, so it plays here and cannot hand off to Spotify.
  const name = await phone.playRow(1);
  await phone.openSheet();
  await phone.advance(5);

  const was = phone.page.url();
  await phone.failAudio({ code: 2, message: 'Network error' });
  await expect(phone.page.locator('.toast')).toBeVisible();
  await phone.page.waitForTimeout(1_000);

  // Same song, same screen, and the sheet still describes it.
  await expect(phone.page.locator('.mini .name')).toHaveText(name);
  await expect(phone.page.locator('.sheet .big')).toHaveText(name);
  expect(phone.page.url()).toBe(was);
  expect(new URL(phone.page.url()).origin).toBe(phone.server.url);
});

// fixme: the phone's queue and position only live in memory, so a reload forgets them.
test.fixme('a reload brings back the paused queue and its place', async ({ phone }) => {
  await phone.open();
  await phone.openMix();

  const name = await phone.playRow(5);
  const queue = (await phone.page.locator('main .row .name').allInnerTexts()).map(line => line.trim());

  await phone.openSheet();
  await phone.page.locator('.sheet .main').click();
  await expect.poll(async () => (await phone.audio())!.paused).toBe(true);
  await phone.advance(45);
  expect((await phone.audio())!.currentTime).toBeGreaterThanOrEqual(45);

  await phone.page.reload();
  await expect(phone.page.locator('nav.tabs')).toBeVisible();

  await expect(phone.page.locator('.mini .name')).toHaveText(name);
  const audio = await phone.audio();
  expect(audio).not.toBeNull();
  expect(audio!.paused).toBe(true);
  expect(audio!.currentTime).toBeGreaterThanOrEqual(44);
  expect(audio!.currentTime).toBeLessThanOrEqual(46);

  await phone.openSheet();
  await expect(phone.page.locator('.sheet .upnext .row .name').first()).toHaveText(queue[6]!);
});

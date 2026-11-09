import { test, expect } from './fixtures.js';

test('the lock screen can play, pause, skip, and not jump', async ({ phone }) => {
  await phone.open();
  await phone.openMix();

  const first = await phone.playRow(0);
  const second = (await phone.row(1).locator('.name').innerText()).trim();
  const session = await phone.mediaSession();

  expect(session.handlers.play).toBe('set');
  expect(session.handlers.pause).toBe('set');
  expect(session.handlers.nexttrack).toBe('set');
  expect(session.handlers.previoustrack).toBe('set');
  // No jump buttons on this player: skipping is only ever to the next song.
  expect(session.handlers.seekforward).toBeNull();
  expect(session.handlers.seekbackward).toBeNull();

  expect(session.metadata).not.toBeNull();
  expect(session.metadata!.title).toBe(first);
  expect(session.metadata!.artist).toBe((await phone.row(0).locator('.by').innerText()).split(' · ')[0]!.trim());
  expect(session.metadata!.artwork?.[0]?.src).toMatch(/^https:\/\//);

  await phone.page.evaluate(() => {
    (window as unknown as { __mediaSession: { call(action: string): unknown } }).__mediaSession.call('nexttrack');
  });
  await expect(phone.page.locator('.mini .name')).toHaveText(second);

  await phone.page.evaluate(() => {
    (window as unknown as { __mediaSession: { call(action: string): unknown } }).__mediaSession.call('pause');
  });
  await expect.poll(async () => (await phone.audio())!.paused).toBe(true);

  await phone.page.evaluate(() => {
    (window as unknown as { __mediaSession: { call(action: string): unknown } }).__mediaSession.call('play');
  });
  await expect.poll(async () => (await phone.audio())!.paused).toBe(false);
});

test('the like stays filled while the phone plays', async ({ phone }) => {
  await phone.open();
  await phone.openMix();
  await phone.playRow(0);
  await phone.openSheet();

  const heart = phone.page.locator('.sheet .heart');
  await expect(heart).toHaveAttribute('aria-pressed', 'false');
  await heart.click();
  await expect(heart).toHaveAttribute('aria-pressed', 'true');

  // It doesn't empty again while the song plays on.
  await phone.page.waitForTimeout(1_500);
  await phone.advance(2);
  await expect(heart).toHaveAttribute('aria-pressed', 'true');
});

test('shuffle and repeat change what the player shows', async ({ phone }) => {
  await phone.open();
  await phone.openMix();
  await phone.playRow(0);
  await phone.openSheet();

  const shuffle = phone.page.locator('.sheet button[aria-label="Shuffle"]');
  await expect(shuffle).toHaveAttribute('aria-pressed', 'false');
  await shuffle.click();
  await expect(shuffle).toHaveAttribute('aria-pressed', 'true');
  await phone.page.waitForTimeout(1_500);
  await expect(shuffle).toHaveAttribute('aria-pressed', 'true');

  const repeat = phone.page.locator('.sheet button[aria-label^="Repeat"]');
  await expect(repeat).toHaveAttribute('aria-pressed', 'false');
  await repeat.click();
  await expect(repeat).toHaveAttribute('aria-pressed', 'true');
  await phone.page.waitForTimeout(1_500);
  await expect(repeat).toHaveAttribute('aria-pressed', 'true');
});

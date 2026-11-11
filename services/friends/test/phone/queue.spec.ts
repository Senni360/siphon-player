import { test, expect } from './fixtures.js';
import type { Page } from '@playwright/test';

const queued = (page: Page) => page.locator('.sheet .upnext .row .name');
const names = async (page: Page, amount: number) =>
  (await queued(page).allInnerTexts()).slice(0, amount).map(name => name.trim());

test('play next puts a song right after the one playing, and nowhere else', async ({ phone }) => {
  await phone.open();
  await phone.openMix();
  await phone.playRow(0);
  const second = (await phone.row(1).locator('.name').innerText()).trim();
  const picked = (await phone.row(20).locator('.name').innerText()).trim();

  await phone.row(20).locator('button.more').click();
  await phone.page.getByRole('button', { name: 'Play next' }).click();
  await expect(phone.page.locator('.toast')).toContainText('Playing next');

  // The phone's own queue took it: nothing was sent to the computer's queue.
  expect(phone.server.sent.filter(command => command.kind === 'queue')).toEqual([]);

  await phone.openSheet();
  await phone.openQueue();
  expect(await names(phone.page, 2)).toEqual([picked, second]);
});

test('a queued song moves, and the order is kept after a reload', async ({ phone }) => {
  await phone.open();
  await phone.openMix();
  await phone.playRow(0);
  await phone.openSheet();
  await phone.openQueue();
  const [first, second, third] = await names(phone.page, 3);

  // The handle is a button too: an arrow key moves the song one place.
  await phone.page.locator('.sheet .upnext .row .handle').first().press('ArrowDown');
  await expect.poll(() => names(phone.page, 3)).toEqual([second, first, third]);

  await phone.page.reload();
  await expect(phone.page.locator('nav.tabs')).toBeVisible();
  await phone.openSheet();
  await phone.openQueue();
  expect(await names(phone.page, 3)).toEqual([second, first, third]);
});

test('a removed song comes back with Undo, in its own place', async ({ phone }) => {
  await phone.open();
  await phone.openMix();
  await phone.playRow(0);
  await phone.openSheet();
  await phone.openQueue();
  const before = await names(phone.page, 4);

  // Pushed off to the left, the second song is taken out.
  await swipe(phone.page, '.sheet .upnext .row:nth-child(2) .words', -260, 0);
  await expect.poll(() => names(phone.page, 3)).toEqual([before[0], before[2], before[3]]);
  await expect(phone.page.locator('.toast')).toContainText(`Removed ${before[1]}`);

  await phone.page.locator('.toast button', { hasText: 'Undo' }).click();
  await expect.poll(() => names(phone.page, 4)).toEqual(before);
});

test('pressing a queued song jumps to it and keeps the rest', async ({ phone }) => {
  await phone.open();
  await phone.openMix();
  await phone.playRow(0);
  await phone.openSheet();
  await phone.openQueue();
  const before = await names(phone.page, 5);

  await phone.page.locator('.sheet .upnext .row .words').nth(2).click();
  await expect(phone.page.locator('.sheet .big')).toHaveText(before[2]!);
  await expect.poll(() => names(phone.page, 2)).toEqual([before[3], before[4]]);
});

test('clearing the queue can be undone', async ({ phone }) => {
  await phone.open();
  await phone.openMix();
  await phone.playRow(0);
  await phone.openSheet();
  await phone.openQueue();
  const before = await names(phone.page, 3);

  await phone.page.locator('.sheet .queue-head button', { hasText: 'Clear' }).click();
  await expect(phone.page.locator('.sheet .upnext')).toContainText('Nothing queued.');
  await phone.page.locator('.toast button', { hasText: 'Undo' }).click();
  await expect.poll(() => names(phone.page, 3)).toEqual(before);
});

test('a swipe on the cover goes to the next song, and back', async ({ phone }) => {
  await phone.open();
  await phone.openMix();
  const first = await phone.playRow(1);
  const second = (await phone.row(2).locator('.name').innerText()).trim();
  await phone.openSheet();

  await swipe(phone.page, '.sheet .cover-box', -220, 0);
  await expect(phone.page.locator('.sheet .big')).toHaveText(second);

  await swipe(phone.page, '.sheet .cover-box', 220, 0);
  await expect(phone.page.locator('.sheet .big')).toHaveText(first);

  // A short, slow nudge is not a skip.
  await swipe(phone.page, '.sheet .cover-box', -30, 0, 400);
  await phone.page.waitForTimeout(400);
  await expect(phone.page.locator('.sheet .big')).toHaveText(first);
});

test('a swipe on the mini player skips without opening it', async ({ phone }) => {
  await phone.open();
  await phone.openMix();
  await phone.playRow(1);
  const second = (await phone.row(2).locator('.name').innerText()).trim();

  await swipe(phone.page, '.mini .open', -200, 0);
  await expect(phone.page.locator('.mini .name')).toHaveText(second);
  await expect(phone.page.locator('.sheet')).toHaveCount(0);
});

test('pulling the sheet down closes it, and a small pull does not', async ({ phone }) => {
  await phone.open();
  await phone.openMix();
  await phone.playRow(0);
  await phone.openSheet();

  await swipe(phone.page, '.sheet header', 0, 60, 400);
  await phone.page.waitForTimeout(400);
  await expect(phone.page.locator('.sheet')).toBeVisible();

  await swipe(phone.page, '.sheet header', 0, 420);
  await expect(phone.page.locator('.sheet')).toHaveCount(0);
  // The page under it is where it was left.
  await expect(phone.page.locator('h1.title')).toHaveText('Everything, 200 Songs');
});

test('dragging the bar under the song seeks, and never closes the sheet', async ({ phone }) => {
  await phone.open();
  await phone.openMix();
  await phone.playRow(0);
  await phone.openSheet();

  const bar = phone.page.locator('.sheet .seek .slider');
  const box = (await bar.boundingBox())!;
  await phone.page.mouse.move(box.x + 4, box.y + box.height / 2);
  await phone.page.mouse.down();
  await phone.page.mouse.move(box.x + box.width / 2, box.y + box.height / 2 + 80, { steps: 6 });
  await phone.page.mouse.up();

  await expect(phone.page.locator('.sheet')).toBeVisible();
  // Half-way along the bar is half-way through the song, as the bar itself counts it.
  const length = Number(await bar.getAttribute('aria-valuemax')) / 1000;
  const audio = await phone.audio();
  expect(audio!.currentTime).toBeGreaterThan(length * 0.4);
  expect(audio!.currentTime).toBeLessThan(length * 0.6);
});

test('search answers as the words are typed, with nothing to press', async ({ phone, server }) => {
  await phone.open();
  await phone.goTab('search');
  const before = server.sent.filter(command => command.kind === 'search').length;

  await phone.page.getByRole('searchbox', { name: 'Search' }).pressSequentially('neon', { delay: 40 });
  await expect(phone.page.locator('main .row').first()).toBeVisible();
  // Four letters typed in one go ask once, not four times.
  expect(server.sent.filter(command => command.kind === 'search').length - before).toBe(1);
});

test('where it plays is chosen in Settings and in the player alike', async ({ phone }) => {
  await phone.open();
  await phone.goTab('library');
  await phone.page.getByRole('button', { name: 'Settings' }).click();
  await expect(phone.page.locator('h1.title')).toHaveText('Settings');

  const onPhone = phone.page.getByRole('button', { name: /^This phone/ });
  const onComputer = phone.page.getByRole('button', { name: /^Your computer/ });
  await expect(onPhone).toHaveAttribute('aria-pressed', 'true');
  await onComputer.click();
  await expect(onComputer).toHaveAttribute('aria-pressed', 'true');
  expect(await phone.page.evaluate(() => localStorage.getItem('encore.target'))).toBe('computer');

  // A phone's volume is its own buttons: the player has no slider for it while it plays here.
  await onPhone.click();
  await expect(onPhone).toHaveAttribute('aria-pressed', 'true');
});

/** A finger dragged across an element, as touch events the page itself would get. */
async function swipe(page: Page, selector: string, dx: number, dy: number, ms = 120): Promise<void> {
  await page.locator(selector).first().evaluate(async (node, move) => {
    const box = node.getBoundingClientRect();
    const fromX = box.left + box.width / 2;
    const fromY = box.top + Math.min(box.height / 2, 40);
    const steps = 8;
    // WebKit will not build a Touch from script, so the event carries plain points:
    // the page reads only where the finger is.
    const fire = (type: string, x: number, y: number) => {
      const point = { identifier: 1, target: node, clientX: x, clientY: y, pageX: x, pageY: y };
      const live = type === 'touchend' ? [] : [point];
      const event = new Event(type, { bubbles: true, cancelable: true });
      Object.defineProperties(event, { touches: { value: live }, targetTouches: { value: live }, changedTouches: { value: [point] } });
      node.dispatchEvent(event);
    };
    fire('touchstart', fromX, fromY);
    for (let step = 1; step <= steps; step++) {
      await new Promise(resolve => setTimeout(resolve, move.ms / steps));
      fire('touchmove', fromX + (move.dx * step) / steps, fromY + (move.dy * step) / steps);
    }
    fire('touchend', fromX + move.dx, fromY + move.dy);
  }, { dx, dy, ms });
}

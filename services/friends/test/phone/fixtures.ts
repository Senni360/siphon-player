// Playwright fixtures for the phone web app: one mock service per test, a
// signed-in browser, fake audio, and a guard that keeps the page on the app.
//
// Each test also checks, at teardown, that the page logged no console errors,
// threw nothing and rejected no promise.
import { test as base, expect, type Locator, type Page, type Route } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { PhoneServer } from './server.js';

const here = dirname(fileURLToPath(import.meta.url));
const FAKE_BROWSER = readFileSync(join(here, 'browser-fake.js'), 'utf8');

const SECRET = 'a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90';
const SEED = {
  'encore.secret': SECRET,
  'encore.name': 'Sam',
  'encore.group': 'group-1',
  'encore.target': 'phone',
  'encore.install': '1',
};
const SEED_SCRIPT = `(() => {
  const seed = ${JSON.stringify(SEED)};
  for (const [key, value] of Object.entries(seed)) localStorage.setItem(key, value);
})();`;

/** A 1x1 transparent PNG, for artwork the app loads from elsewhere. */
const PIXEL = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==',
  'base64',
);

/** Anything that leaves the app's origin is a failed test, unless allowed once. */
export class NavigationGuard {
  attempts: string[] = [];
  violations: string[] = [];
  allowedUrls: string[] = [];
  private allowed: RegExp | null = null;

  constructor(readonly origin: string) {}

  /** The one deliberate way out: a test exercising Spotify's own sign-in. */
  allowNext(pattern: RegExp): void {
    this.allowed = pattern;
  }

  consume(url: string): boolean {
    if (!this.allowed || !this.allowed.test(url)) return false;
    this.allowed = null;
    this.allowedUrls.push(url);
    return true;
  }

  attach(page: Page): void {
    page.on('framenavigated', frame => {
      if (frame !== page.mainFrame()) return;
      const url = frame.url();
      if (url.startsWith(this.origin) || url === 'about:blank' || url.startsWith('data:')) return;
      const bare = (value: string) => {
        try { const parsed = new URL(value); return parsed.origin + parsed.pathname; } catch { return value; }
      };
      if (this.allowedUrls.some(allowed => bare(allowed) === bare(url))) return;
      this.violations.push(url);
    });
    // An app scheme (spotify:, intent:, ...) never reaches the network, so the
    // browser's own refusal to launch it is the record of the attempt.
    page.on('console', message => {
      const launch = /(?:Failed to launch|Not allowed to launch) '([^']+)'/.exec(message.text());
      if (launch) this.violations.push(launch[1]!);
    });
  }
}

/**
 * Every request that isn't the app's stays here. Artwork gets a pixel, Spotify's
 * API gets a plausible answer, and a page that tries to leave is recorded (and
 * shown a stub) rather than reaching the network.
 */
async function serveExternal(route: Route, server: PhoneServer, guard: NavigationGuard): Promise<void> {
  const request = route.request();
  const url = request.url();
  if (url.startsWith(server.url)) {
    await route.fallback();
    return;
  }

  let leaving = false;
  try {
    leaving = request.isNavigationRequest() && request.frame() === request.frame().page().mainFrame();
  } catch { /* no frame: a worker or a service worker */ }

  if (leaving) {
    if (guard.consume(url)) {
      await route.fulfill({
        status: 200,
        contentType: 'text/html; charset=utf-8',
        body: '<!doctype html><html lang="en"><head><title>Spotify</title></head><body>Spotify sign-in</body></html>',
      });
      return;
    }
    guard.attempts.push(url);
    await route.fulfill({
      status: 200,
      contentType: 'text/html; charset=utf-8',
      body: '<!doctype html><html lang="en"><head><title>Left the app</title></head><body>This page should not have been left.</body></html>',
    });
    return;
  }

  const parsed = new URL(url);
  if (parsed.hostname === 'accounts.spotify.com') {
    if (parsed.pathname === '/api/token') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ access_token: 'test-access', refresh_token: 'test-refresh', expires_in: 3600, token_type: 'Bearer' }),
      });
      return;
    }
    await route.fulfill({ status: 200, contentType: 'text/html; charset=utf-8', body: '<!doctype html><title>Spotify</title>' });
    return;
  }
  if (parsed.hostname === 'api.spotify.com') {
    await route.fulfill({ status: 200, contentType: 'application/json', body: '{}' });
    return;
  }
  if (request.resourceType() === 'image') {
    await route.fulfill({ status: 200, contentType: 'image/png', body: PIXEL });
    return;
  }
  await route.fulfill({ status: 204, body: '' });
}

export interface AudioSnapshot {
  src: string;
  currentTime: number;
  duration: number;
  paused: boolean;
  ended: boolean;
  volume: number;
  muted: boolean;
  error: { code: number; message: string } | null;
  readyState: number;
  playCalls: number;
  pauseCalls: number;
}

export interface SessionSnapshot {
  handlers: Record<string, 'set' | null>;
  metadata: { title?: string; artist?: string; album?: string; artwork?: Array<{ src: string; sizes?: string }> } | null;
  positions: unknown[];
  calls: string[];
}

export interface Phone {
  page: Page;
  server: PhoneServer;
  guard: NavigationGuard;
  open(): Promise<void>;
  goTab(tab: 'home' | 'search' | 'library' | 'group'): Promise<void>;
  openMix(): Promise<void>;
  row(index: number): Locator;
  playRow(index: number): Promise<string>;
  openSheet(): Promise<void>;
  openQueue(): Promise<void>;
  audio(): Promise<AudioSnapshot | null>;
  mediaSession(): Promise<SessionSnapshot>;
  advance(seconds: number): Promise<void>;
  failAudio(details?: { code?: number; message?: string }): Promise<void>;
  audioCleared(): Promise<boolean>;
}

function makePhone(page: Page, server: PhoneServer, guard: NavigationGuard): Phone {
  const phone: Phone = {
    page,
    server,
    guard,

    async open() {
      await page.goto(`${server.url}/`);
      await expect(page.locator('nav.tabs')).toBeVisible();
      // No banner once the computer's state has landed; the install tip is
      // seeded away, so a banner here means the page thinks the computer is away.
      await expect(page.locator('.banner')).toHaveCount(0, { timeout: 20_000 });
    },

    async goTab(tab) {
      await page.locator(`nav.tabs button[data-page="${tab}"]`).click();
    },

    async openMix() {
      await phone.goTab('library');
      await page.locator('main .row').filter({ hasText: 'Everything, 200 Songs' }).locator('button.words').click();
      await expect(page.locator('h1.title')).toHaveText('Everything, 200 Songs');
      await expect(page.locator('main .row')).toHaveCount(50);
    },

    row(index) {
      return page.locator('main .row').nth(index);
    },

    async playRow(index) {
      const row = phone.row(index);
      const name = (await row.locator('.name').innerText()).trim();
      await row.locator('button.words').click();
      await expect(page.locator('.mini')).toBeVisible();
      await expect(page.locator('.mini .name')).toHaveText(name);
      await expect.poll(async () => (await phone.audio())?.src ?? '', { timeout: 20_000 }).toMatch(/^https:/);
      await expect.poll(async () => (await phone.audio())?.paused, { timeout: 20_000 }).toBe(false);
      return name;
    },

    async openSheet() {
      await page.locator('.mini button.open').click();
      await expect(page.locator('.sheet')).toBeVisible();
      // It rises into place over a moment. A finger waits for that; a mouse drag
      // placed by coordinates does not, so wait here or it lands mid-animation.
      await expect.poll(async () => page.evaluate(() => {
        const sheet = document.querySelector('.sheet');
        return sheet ? Math.round(sheet.getBoundingClientRect().top) : -1;
      }), { message: 'the sheet settles into place' }).toBeLessThanOrEqual(1);
    },

    async openQueue() {
      // The queue is one of the three things the middle of the sheet can show.
      await page.locator('.sheet .tools button', { hasText: 'Queue' }).click();
      await expect(page.locator('.sheet .upnext')).toBeVisible();
    },

    async audio() {
      return page.evaluate(() => {
        const audio = (window as unknown as { __audio?: { current(): AudioSnapshot | null } }).__audio;
        return audio ? audio.current() : null;
      });
    },

    async mediaSession() {
      return page.evaluate(() => {
        const session = (window as unknown as {
          __mediaSession: {
            handlers: Record<string, unknown>;
            metadata: SessionSnapshot['metadata'];
            positions: unknown[];
            calls: string[];
          };
        }).__mediaSession;
        const handlers: Record<string, 'set' | null> = {};
        for (const action of ['play', 'pause', 'nexttrack', 'previoustrack', 'seekforward', 'seekbackward', 'seekto', 'stop']) {
          handlers[action] = typeof session.handlers[action] === 'function' ? 'set' : null;
        }
        return { handlers, metadata: session.metadata, positions: session.positions, calls: session.calls };
      });
    },

    async advance(seconds) {
      await page.evaluate(value => {
        (window as unknown as { __audio: { advance(by: number): unknown } }).__audio.advance(value);
      }, seconds);
    },

    async failAudio(details) {
      await page.evaluate(given => {
        (window as unknown as { __audio: { fail(details?: unknown): unknown } }).__audio.fail(given);
      }, details ?? {});
    },

    async audioCleared() {
      return page.evaluate(() => {
        const audio = (window as unknown as { __audio?: { all(): AudioSnapshot[] } }).__audio;
        return !!audio && audio.all().every(element => element.src === '');
      });
    },
  };
  return phone;
}

interface Observed {
  consoleErrors: string[];
  pageErrors: string[];
  /**
   * A browser-level log line this test knowingly causes, such as the console's
   * own note about a deliberately injected HTTP 500. JavaScript console errors
   * are never allowed.
   */
  allowConsole(pattern: RegExp): void;
  /** A browser-reported page error this test knowingly causes. */
  allowPageError(pattern: RegExp): void;
}

type Fixtures = {
  server: PhoneServer;
  guard: NavigationGuard;
  observed: Observed;
  phone: Phone;
};

export const test = base.extend<Fixtures>({
  server: async ({}, use) => {
    const server = new PhoneServer();
    await server.start();
    await use(server);
    await server.close();
  },

  guard: async ({ server }, use) => {
    await use(new NavigationGuard(server.url));
  },

  // The page is prepared here, before any test navigates it: the fake browser
  // pieces, the stored sign-in, and the routes that keep everything local.
  page: async ({ context, page, server, guard }, use) => {
    await context.addInitScript({ content: FAKE_BROWSER });
    await context.addInitScript({ content: SEED_SCRIPT });
    await context.route('**/*', route => serveExternal(route, server, guard));
    guard.attach(page);
    await use(page);
  },

  phone: async ({ page, server, guard }, use) => {
    await use(makePhone(page, server, guard));
  },

  // Every test fails on console errors, page errors or unhandled rejections,
  // and on any attempt to leave the app's origin.
  observed: [async ({ page, guard }, use) => {
    const allowed: RegExp[] = [];
    const allowedPage: RegExp[] = [];
    const seen: Observed = {
      consoleErrors: [],
      pageErrors: [],
      allowConsole(pattern) { allowed.push(pattern); },
      allowPageError(pattern) { allowedPage.push(pattern); },
    };
    page.on('console', message => {
      if (message.type() === 'error') seen.consoleErrors.push(message.text());
    });
    page.on('pageerror', error => seen.pageErrors.push(error.message));

    await use(seen);

    const unhandled = await page.evaluate(() => {
      const book = (window as unknown as { __phoneTest?: { unhandled?: string[] } }).__phoneTest;
      return book?.unhandled ?? [];
    }).catch(() => [] as string[]);
    const opened = await page.evaluate(() => {
      const book = (window as unknown as { __phoneTest?: { external?: string[] } }).__phoneTest;
      return book?.external ?? [];
    }).catch(() => [] as string[]);

    const unexpected = seen.consoleErrors.filter(text => !allowed.some(pattern => pattern.test(text)));
    const unexpectedPage = seen.pageErrors.filter(text => !allowedPage.some(pattern => pattern.test(text)));
    expect(unexpected, 'console errors').toEqual([]);
    expect(unexpectedPage, 'page errors').toEqual([]);
    expect(unhandled, 'unhandled rejections').toEqual([]);
    expect(opened, 'window.open calls').toEqual([]);
    expect(guard.violations, 'navigation away from the app origin').toEqual([]);
  }, { auto: true }],
});

export { expect };

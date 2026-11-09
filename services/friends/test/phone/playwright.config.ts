import { defineConfig, devices } from '@playwright/test';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

// The phone app's browser tests. Run them with `npm run test:phone` from
// services/friends. They use Playwright's own WebKit and Chromium: no real
// network, no real audio, and no account.
export default defineConfig({
  testDir: here,
  outputDir: join(here, 'test-results'),
  timeout: 60_000,
  expect: { timeout: 10_000 },
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: process.env.CI ? 2 : 4,
  reporter: [['list']],
  use: {
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'off',
  },
  projects: [
    { name: 'iPhone 13', use: { ...devices['iPhone 13'] } },
    { name: 'iPhone SE', use: { ...devices['iPhone SE'] } },
    { name: 'Pixel 7', use: { ...devices['Pixel 7'] } },
  ],
});

import {defineConfig} from '@playwright/test';

export default defineConfig({
  testDir: './tests/browser',
  timeout: 120_000,
  expect: {timeout: 60_000},
  workers: 1,
  use: {baseURL: 'http://127.0.0.1:4173/wavelet/', headless: true},
  webServer: {
    command: 'npm run serve -- --host 127.0.0.1 --port 4173 --no-open',
    url: 'http://127.0.0.1:4173/wavelet/',
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
});

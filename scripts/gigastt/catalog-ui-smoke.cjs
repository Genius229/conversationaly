// Browser integration test of the built UI with a deterministic Tauri IPC fixture.
// This tests UI wiring, not native model downloads or recognition.
// Build frontend/out first; a loopback-only static server is started automatically.
// PLAYWRIGHT_MODULE=/path/to/playwright node scripts/gigastt/catalog-ui-smoke.cjs
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { serveBuiltUI, installTauriFixture } = require('./ui-smoke-support.cjs');

const artifacts = process.env.CATALOG_UI_ARTIFACTS || '/tmp/gigastt-catalog-ui';
fs.mkdirSync(artifacts, { recursive: true });

async function main() {
  const server = process.env.CATALOG_UI_BASE_URL ? null : await serveBuiltUI();
  const base = process.env.CATALOG_UI_BASE_URL || `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch({
    executablePath: process.env.CHROMIUM_PATH || '/usr/bin/chromium',
    headless: true,
  });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = [];
  const consoleErrors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') consoleErrors.push(message.text()); });
  await page.addInitScript(installTauriFixture);

  try {
    await page.goto(`${base}/settings`);
    await page.getByRole('tab', { name: 'Transcription', exact: true }).click();
    const catalogCard = page.getByRole('region', { name: 'GigaSTT model', exact: true });
    await catalogCard.waitFor({ state: 'visible', timeout: 15000 });
    await page.evaluate(() => document.fonts.ready);
    await page.waitForTimeout(300); // Let the existing tab indicator settle for screenshots.
    await page.screenshot({ path: path.join(artifacts, 'desktop-missing.png'), fullPage: true });
    const search = page.getByRole('searchbox', { name: 'Search transcription models', exact: true });
    await search.fill('gigastt');
    assert.equal(await catalogCard.count(), 1, 'GigaSTT must participate in search');
    await search.fill('no-such-model');
    await catalogCard.waitFor({ state: 'detached' });
    assert.equal(await catalogCard.count(), 0, 'Unmatched query must hide GigaSTT');
    await search.fill('');
    const installedOnly = page.getByRole('switch', { name: 'Show only installed models', exact: true });
    await installedOnly.click();
    await catalogCard.waitFor({ state: 'detached' });
    assert.equal(await catalogCard.count(), 0, 'Missing GigaSTT is not installed');
    await installedOnly.click();
    await catalogCard.getByRole('button', { name: /download/i }).click();
    await catalogCard.getByRole('button', { name: /cancel/i }).waitFor();
    await page.screenshot({ path: path.join(artifacts, 'desktop-downloading.png'), fullPage: true });
    await catalogCard.getByRole('button', { name: /cancel/i }).click();
    await catalogCard.getByRole('button', { name: /download|repair|retry/i }).click();
    await page.evaluate(() => window.__catalogMock.fail());
    await catalogCard.getByText('Test download interrupted', { exact: false }).waitFor();
    await catalogCard.getByRole('button', { name: /download|repair|retry/i }).click();
    await page.evaluate(() => window.__catalogMock.complete());
    await catalogCard.getByText(/ready|installed/i).first().waitFor();
    await installedOnly.click();
    await catalogCard.waitFor();
    await page.setViewportSize({ width: 720, height: 900 });
    await page.screenshot({ path: path.join(artifacts, 'narrow-ready.png'), fullPage: true });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), 'No horizontal overflow');
    const sortBox = await page.getByRole('combobox', { name: 'Sort models by', exact: true }).boundingBox();
    assert.ok(sortBox && sortBox.x >= 0 && sortBox.x + sortBox.width <= 721, 'Sort control remains inside the minimum-width window');

    await page.goto(`${base}/meeting-details?id=catalog-smoke`);
    const meetingPanel = page.getByRole('region', { name: 'GigaSTT final transcription' });
    await meetingPanel.getByText('Ready', { exact: true }).waitFor();
    assert.equal(await meetingPanel.getByRole('button', { name: /download.*models/i }).count(), 0);
    await page.goto(`${base}/settings`);
    await page.getByRole('tab', { name: 'Transcription', exact: true }).click();
    await catalogCard.waitFor();
    const counters = await page.evaluate(() => ({ ...window.__catalogMock.state }));
    assert.equal(counters.downloads, 3, 'Only explicit user actions install models');
    assert.equal(counters.cancels, 1);
    assert.equal(counters.providerWrites, 0, 'GigaSTT download must not select a legacy live model');
    assert.deepEqual(errors, [], 'No uncaught browser errors');
    assert.deepEqual(consoleErrors, [], 'No browser console errors');
    fs.rmSync(path.join(artifacts, 'failure.txt'), { force: true });
    fs.rmSync(path.join(artifacts, 'failure.png'), { force: true });
    fs.writeFileSync(path.join(artifacts, 'console-errors.json'), JSON.stringify(consoleErrors, null, 2));
    console.log(JSON.stringify({ result: 'PASS', checks: 'catalog/search/installed/download/cancel/error/retry/shared-meeting-state/responsive', artifacts, consoleErrors }));
  } catch (error) {
    await page.screenshot({ path: path.join(artifacts, 'failure.png'), fullPage: true });
    const unknown = await page.evaluate(() => [...window.__catalogMock?.unknownCommands || []]);
    fs.writeFileSync(path.join(artifacts, 'failure.txt'), `${error.stack}\nBrowser errors:\n${errors.join('\n')}\nConsole errors:\n${consoleErrors.join('\n')}\nUnstubbed IPC:\n${unknown.join('\n')}\nBody:\n${await page.locator('body').innerText()}`);
    throw error;
  } finally {
    await browser.close();
    if (server) {
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
    }
  }
}
main().catch(error => { console.error(error.message); process.exitCode = 1; });

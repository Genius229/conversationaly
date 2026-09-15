// Real built-page VAD setting interactions with deterministic Tauri IPC.
// Does not claim native ASR quality or physical Windows acceptance.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { serveBuiltUI, installTauriFixture } = require('./ui-smoke-support.cjs');

const artifacts = process.env.VAD_UI_ARTIFACTS || '/tmp/gigastt-vad-ui';
fs.mkdirSync(artifacts, { recursive: true });

function installVadSettingsFixture() {
  const key = 'vad-settings-smoke';
  const state = JSON.parse(sessionStorage.getItem(key) || 'null') || {
    settings: { auto_transcribe: true, live_preview: false, vad_enabled: true },
    writes: [], failNext: false, failures: 0,
  };
  const persist = () => sessionStorage.setItem(key, JSON.stringify(state));
  const original = window.__TAURI_INTERNALS__.invoke;
  window.__vadSettingsMock = state;
  window.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
    if (command === 'gigastt_get_settings') return structuredClone(state.settings);
    if (command === 'gigastt_save_settings') {
      state.writes.push(structuredClone(args.settings));
      if (state.failNext) {
        state.failNext = false;
        state.failures++;
        persist();
        throw new Error('Test VAD settings save failed');
      }
      state.settings = structuredClone(args.settings);
      persist();
      return null;
    }
    return original(command, args);
  };
}

async function checked(page, locator, expected) {
  await page.waitForFunction(({ element, expected }) =>
    element.getAttribute('aria-checked') === String(expected),
  { element: await locator.elementHandle(), expected });
}

async function main() {
  const server = await serveBuiltUI();
  const browser = await chromium.launch({
    executablePath: process.env.CHROMIUM_PATH || '/usr/bin/chromium', headless: true,
  });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript({ content:
    `(${installTauriFixture.toString()})({ modelsReady: true }); (${installVadSettingsFixture.toString()})();` });
  try {
    await page.goto(`http://127.0.0.1:${server.address().port}/meeting-details?id=vad-smoke`);
    const panel = page.getByRole('region', { name: 'GigaSTT final transcription' });
    const toggle = panel.getByRole('switch', { name: 'GigaSTT VAD', exact: true });
    await toggle.waitFor({ state: 'visible', timeout: 15000 });
    await checked(page, toggle, true);
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: path.join(artifacts, 'desktop-on.png'), fullPage: true });

    await toggle.click();
    await checked(page, toggle, false);
    let state = await page.evaluate(() => window.__vadSettingsMock);
    assert.deepEqual(state.settings, { auto_transcribe: true, live_preview: false, vad_enabled: false });
    assert.equal(state.writes.length, 1, 'One explicit interaction produces one save');
    await page.reload();
    await checked(page, toggle, false);

    await toggle.focus();
    await page.keyboard.press('Space');
    await checked(page, toggle, true);
    state = await page.evaluate(() => window.__vadSettingsMock);
    assert.equal(state.writes.length, 2, 'Keyboard interaction is saved once');
    assert.equal(state.settings.auto_transcribe, true);
    assert.equal(state.settings.live_preview, false);

    await page.evaluate(() => { window.__vadSettingsMock.failNext = true; });
    await toggle.click();
    await page.waitForFunction(() => window.__vadSettingsMock.failures === 1);
    await checked(page, toggle, true);
    await page.getByText(/Test VAD settings save failed/).first().waitFor();
    state = await page.evaluate(() => window.__vadSettingsMock);
    assert.equal(state.settings.vad_enabled, true, 'Failed save does not alter the persisted choice');
    await page.reload();
    await checked(page, toggle, true);

    await page.setViewportSize({ width: 720, height: 900 });
    await page.screenshot({ path: path.join(artifacts, 'narrow-on.png'), fullPage: true });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), 'No horizontal overflow');
    const box = await toggle.boundingBox();
    assert.ok(box && box.x >= 0 && box.x + box.width <= 721, 'Toggle remains inside the minimum-width window');
    assert.deepEqual(errors, [], 'No uncaught browser errors');
    fs.writeFileSync(path.join(artifacts, 'result.json'), JSON.stringify({ result: 'PASS', checks: 'default-on/off/save/reload/keyboard/error-rollback/other-settings/responsive', errors }, null, 2));
    console.log(JSON.stringify({ result: 'PASS', artifacts, errors }));
  } catch (error) {
    await page.screenshot({ path: path.join(artifacts, 'failure.png'), fullPage: true });
    fs.writeFileSync(path.join(artifacts, 'failure.txt'), `${error.stack}\n${errors.join('\n')}\n${await page.locator('body').innerText()}`);
    throw error;
  } finally {
    await browser.close();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
}
main().catch(error => { console.error(error.message); process.exitCode = 1; });

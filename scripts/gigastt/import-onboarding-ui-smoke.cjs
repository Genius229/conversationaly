// Real built UI + shared deterministic native IPC fixture. No model/network inference.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { serveBuiltUI, installTauriFixture } = require('./ui-smoke-support.cjs');

const artifacts = process.env.GIGASTT_FLOW_ARTIFACTS || '/tmp/gigastt-import-onboarding-ui';
fs.mkdirSync(artifacts, { recursive: true });

async function main() {
  const server = await serveBuiltUI();
  const base = `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || '/usr/bin/chromium' });
  const checks = [];
  const errors = [];
  let page;
  async function scenario(name, options, run) {
    const context = await browser.newContext({ viewport: { width: 1100, height: 800 } });
    page = await context.newPage();
    const pageErrors = [];
    page.on('pageerror', error => pageErrors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') pageErrors.push(message.text()); });
    await page.addInitScript(installTauriFixture, options);
    try {
      await run(page);
      assert.deepEqual(pageErrors, [], `${name}: browser errors`);
      await page.screenshot({ path: path.join(artifacts, `${name}.png`), fullPage: true });
      fs.rmSync(path.join(artifacts, `${name}-failure.txt`), { force: true });
      fs.rmSync(path.join(artifacts, `${name}-failure.png`), { force: true });
      checks.push(name);
    } catch (error) {
      errors.push({ name, error: error.stack, pageErrors });
      await page.screenshot({ path: path.join(artifacts, `${name}-failure.png`), fullPage: true });
      fs.writeFileSync(path.join(artifacts, `${name}-failure.txt`), `${error.stack}\n${pageErrors.join('\n')}\n${await page.locator('body').innerText()}`);
      throw error;
    } finally {
      await context.close();
    }
  }
  async function toModels(page) {
    await page.goto(base);
    await page.getByRole('button', { name: 'Get Started', exact: true }).click();
    await page.getByRole('button', { name: 'Continue', exact: true }).click();
    await page.getByRole('heading', { name: 'Optional model setup', exact: true }).waitFor();
    await page.getByRole('button', { name: 'Set up models later', exact: true }).waitFor();
  }
  const state = page => page.evaluate(() => JSON.parse(sessionStorage.getItem('catalog-ipc')));
  const downloadCount = data => data.downloads + data.liveDownloads + data.summaryDownloads;

  try {
    await scenario('first-run-skip', { fresh: true }, async page => {
      await toModels(page);
      assert.equal(downloadCount(await state(page)), 0);
      await page.screenshot({ path: path.join(artifacts, 'model-setup-idle.png'), fullPage: true });
      await page.getByRole('button', { name: 'Set up models later', exact: true }).click();
      await page.getByRole('button', { name: 'Import audio', exact: true }).waitFor();
      let data = await state(page);
      assert.equal(data.onboardingStatus.completed, true);
      assert.equal(data.onboardingStatus.model_status.summary, 'not_downloaded');
      assert.equal(data.onboardingStatus.model_status.parakeet, 'not_downloaded');
      assert.equal(downloadCount(data), 0);
      await page.reload();
      await page.getByRole('button', { name: 'Import audio', exact: true }).waitFor();
      data = await state(page);
      assert.equal(downloadCount(data), 0, 'Restart must not launch missing model downloads');
    });

    await scenario('explicit-downloads-and-reentry', { fresh: true, livePreview: true }, async page => {
      await toModels(page);
      assert.equal(downloadCount(await state(page)), 0);
      await page.getByRole('button', { name: 'Download live model', exact: true }).click();
      await page.getByRole('button', { name: 'Download summary model', exact: true }).click();
      await page.getByRole('button', { name: 'Continue downloads in background', exact: true }).waitFor();
      await page.screenshot({ path: path.join(artifacts, 'explicit-model-downloads.png'), fullPage: true });
      await page.waitForTimeout(1200); // Allow the existing 1s persistence debounce before WebView reload.
      await page.reload();
      await page.getByRole('button', { name: 'Continue downloads in background', exact: true }).waitFor();
      let data = await state(page);
      assert.equal(data.liveDownloads, 1);
      assert.equal(data.summaryDownloads, 1);
      await page.getByRole('button', { name: 'Continue downloads in background', exact: true }).click();
      await page.getByRole('button', { name: 'Import audio', exact: true }).waitFor();
      data = await state(page);
      assert.equal(data.liveDownloads, 1);
      assert.equal(data.summaryDownloads, 1, 'Finishing setup must not start another download');
      assert.equal(data.onboardingStatus.model_status.summary, 'not_downloaded');
    });

    await scenario('mac-skip-retains-permissions', { fresh: true, platform: 'macos' }, async page => {
      await toModels(page);
      await page.getByRole('button', { name: 'Set up models later', exact: true }).click();
      await page.getByRole('button', { name: 'Finish Setup', exact: true }).waitFor();
      const data = await state(page);
      assert.ok(!data.calls.includes('complete_onboarding'), 'Mac model skip must not bypass Permissions');
      assert.equal(downloadCount(data), 0);
    });

    await scenario('gigastt-import-and-retry', { modelsReady: true, importFailures: 1 }, async page => {
      await page.goto(base);
      await page.getByRole('button', { name: 'Import audio', exact: true }).click();
      await page.getByRole('button', { name: 'Select audio file', exact: true }).click();
      await page.getByLabel('Meeting title', { exact: true }).fill('Imported MP4 test');
      await page.setViewportSize({ width: 720, height: 800 });
      const importButton = page.getByRole('button', { name: 'Import with GigaSTT', exact: true });
      await importButton.scrollIntoViewIfNeeded();
      const importBox = await importButton.boundingBox();
      assert.ok(importBox && importBox.x >= 0 && importBox.x + importBox.width <= 721);
      await page.screenshot({ path: path.join(artifacts, 'import-dialog-narrow.png'), fullPage: true });
      await page.setViewportSize({ width: 1100, height: 800 });
      await page.getByRole('button', { name: 'Import with GigaSTT', exact: true }).click();
      await page.getByRole('button', { name: 'Retry import', exact: true }).waitFor();
      assert.equal(await page.getByLabel('Meeting title', { exact: true }).inputValue(), 'Imported MP4 test');
      await page.getByRole('region', { name: 'Selected audio file', exact: true }).getByText('input.mp4', { exact: true }).waitFor();
      await page.getByRole('button', { name: 'Retry import', exact: true }).click();
      await page.waitForURL('**/meeting-details?id=imported-fixture&source=import');
      const panel = page.getByRole('region', { name: 'GigaSTT final transcription' });
      await panel.getByRole('button', { name: 'Cancel', exact: true }).waitFor();
      let data = await state(page);
      assert.deepEqual(data.importRequests, [
        { sourcePath: '/fixture/input.mp4', title: 'Imported MP4 test' },
        { sourcePath: '/fixture/input.mp4', title: 'Imported MP4 test' },
      ]);
      assert.ok(!data.calls.includes('start_import_audio_command'), 'Import must not use legacy recognition');
      assert.equal(downloadCount(data), 0);
      await page.evaluate(() => window.__catalogMock.finishJob());
      await panel.getByText('Final transcript ready', { exact: true }).waitFor();
      data = await state(page);
      assert.ok(data.calls.includes('api_get_meeting_transcripts'));
    });

    await scenario('stale-file-validation', { modelsReady: true, stallFirstSelection: true }, async page => {
      await page.goto(base);
      await page.getByRole('button', { name: 'Import audio', exact: true }).click();
      await page.getByRole('button', { name: 'Select audio file', exact: true }).click();
      await page.waitForFunction(() => window.__catalogMock.state.calls.includes('select_and_validate_audio_command'));
      await page.keyboard.press('Escape');
      await page.getByRole('dialog').waitFor({ state: 'hidden' });
      await page.getByRole('button', { name: 'Import audio', exact: true }).click();
      await page.getByRole('button', { name: 'Select audio file', exact: true }).click();
      await page.getByRole('region', { name: 'Selected audio file', exact: true }).getByText('second.mp4', { exact: true }).waitFor();
      await page.evaluate(() => window.__catalogMock.releaseSelection());
      await page.waitForTimeout(100);
      assert.equal(await page.getByLabel('Meeting title', { exact: true }).inputValue(), 'second.mp4', 'Closed-dialog validation must not overwrite the new file');
      await page.getByRole('region', { name: 'Selected audio file', exact: true }).getByText('second.mp4', { exact: true }).waitFor();
    });
    fs.writeFileSync(path.join(artifacts, 'result.json'), JSON.stringify({ result: 'PASS', checks, errors }, null, 2));
    console.log(JSON.stringify({ result: 'PASS', checks, artifacts, errors }));
  } finally {
    await browser.close();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
}
main().catch(error => { console.error(error.message); process.exitCode = 1; });

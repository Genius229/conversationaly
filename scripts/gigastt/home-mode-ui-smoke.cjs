// Built-home browser matrix for persisted final-transcript and live-preview settings.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { serveBuiltUI, installTauriFixture } = require('./ui-smoke-support.cjs');

const artifacts = process.env.HOME_MODE_UI_ARTIFACTS || '/tmp/gigastt-home-mode-ui';
fs.mkdirSync(artifacts, { recursive: true });

async function main() {
  const server = await serveBuiltUI();
  const base = `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch({
    executablePath: process.env.CHROMIUM_PATH || '/usr/bin/chromium',
    headless: true,
  });
  const liveModel = 'parakeet-tdt-0.6b-v3-q8';
  const checks = [];
  const errors = [];

  async function scenario(name, options, expected) {
    const context = await browser.newContext({ viewport: { width: 420, height: 800 } });
    const page = await context.newPage();
    const pageErrors = [];
    page.on('pageerror', error => pageErrors.push(error.message));
    page.on('console', message => {
      if (message.type() === 'error') pageErrors.push(message.text());
    });
    await page.addInitScript(installTauriFixture, { liveModel, ...options });

    try {
      await page.goto(base);
      const mode = page.getByLabel(expected.aria, { exact: true });
      await mode.waitFor({ state: 'visible', timeout: 15000 });
      await page.getByText(expected.idle, { exact: true }).waitFor();

      for (const visible of expected.visible) {
        await mode.getByText(visible, { exact: true }).waitFor();
      }
      for (const absent of expected.absent) {
        assert.equal(await page.getByText(absent, { exact: false }).count(), 0, `${name}: unexpected ${absent}`);
      }

      const modeBox = await mode.boundingBox();
      assert.ok(modeBox && modeBox.x >= 0 && modeBox.x + modeBox.width <= 421, `${name}: mode stays inside narrow viewport`);
      assert.ok(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
        `${name}: no horizontal overflow`,
      );
      assert.deepEqual(pageErrors, [], `${name}: browser errors`);

      await page.screenshot({ path: path.join(artifacts, `${name}.png`), fullPage: true });
      checks.push(name);
    } catch (error) {
      errors.push({ name, error: error.stack, pageErrors });
      await page.screenshot({ path: path.join(artifacts, `${name}-failure.png`), fullPage: true });
      fs.writeFileSync(
        path.join(artifacts, `${name}-failure.txt`),
        `${error.stack}\n${pageErrors.join('\n')}\n${await page.locator('body').innerText()}`,
      );
      throw error;
    } finally {
      await context.close();
    }
  }

  try {
    await scenario('auto-final-no-preview', { autoTranscribe: true, livePreview: false }, {
      aria: 'Automatic final transcript: GigaSTT after recording.',
      idle: 'Start a recording. GigaSTT creates the final transcript after recording ends.',
      visible: ['GigaSTT · After recording'],
      absent: [liveModel, 'live draft'],
    });
    await scenario('auto-final-live-preview', { autoTranscribe: true, livePreview: true }, {
      aria: `Automatic final transcript: GigaSTT after recording. Live draft: ${liveModel}.`,
      idle: 'Start a recording. A live draft appears here, then GigaSTT creates the final transcript after recording.',
      visible: ['GigaSTT · After recording', `${liveModel} · Live draft`],
      absent: ['Audio only', 'Automatic transcription is off'],
    });
    await scenario('manual-final-live-preview', { autoTranscribe: false, livePreview: true }, {
      aria: `Live draft: ${liveModel}. Automatic final transcription is off; GigaSTT is available manually.`,
      idle: 'Start a recording to see a live draft. Run GigaSTT from the saved meeting when you want a final transcript.',
      visible: [`${liveModel} · Live draft`, 'GigaSTT · Manual final'],
      absent: ['GigaSTT · After recording', 'Audio only'],
    });
    await scenario('audio-only', { autoTranscribe: false, livePreview: false }, {
      aria: 'Automatic transcription is off. Audio only; GigaSTT is available manually.',
      idle: 'Start a recording to save audio. Run GigaSTT from the saved meeting when you want a transcript.',
      visible: ['Audio only', 'GigaSTT · Manual final'],
      absent: [liveModel, 'Live draft', 'GigaSTT · After recording'],
    });
    await scenario('settings-loading', { settingsDelayMs: 5000 }, {
      aria: 'Loading transcription settings.',
      idle: 'Start a recording to save audio. Transcription follows your saved settings.',
      visible: ['Loading transcription settings…'],
      absent: [liveModel, 'Live draft', 'GigaSTT · After recording'],
    });
    await scenario('settings-error', { settingsError: true }, {
      aria: 'Transcription settings unavailable.',
      idle: 'Start a recording to save audio. Transcription settings could not be verified.',
      visible: ['Transcription settings unavailable'],
      absent: [liveModel, 'Live draft', 'GigaSTT · After recording'],
    });

    fs.writeFileSync(path.join(artifacts, 'result.json'), JSON.stringify({ result: 'PASS', checks, errors }, null, 2));
    console.log(JSON.stringify({ result: 'PASS', checks, artifacts, errors }));
  } finally {
    await browser.close();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
}

main().catch(error => {
  console.error(error.message);
  process.exitCode = 1;
});

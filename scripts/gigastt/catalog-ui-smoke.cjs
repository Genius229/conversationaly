// Browser integration test of the built UI with a deterministic Tauri IPC fixture.
// This tests UI wiring, not native model downloads or recognition.
// Build frontend/out first; a loopback-only static server is started automatically.
// PLAYWRIGHT_MODULE=/path/to/playwright node scripts/gigastt/catalog-ui-smoke.cjs
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');

const artifacts = process.env.CATALOG_UI_ARTIFACTS || '/tmp/gigastt-catalog-ui';
fs.mkdirSync(artifacts, { recursive: true });

async function serveBuiltUI() {
  const root = path.resolve(__dirname, '../../frontend/out');
  assert.ok(fs.existsSync(path.join(root, 'settings.html')), 'Build frontend/out before this test');
  const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2', '.txt': 'text/plain' };
  const server = http.createServer((request, response) => {
    try {
      const pathname = decodeURIComponent(new URL(request.url, 'http://localhost').pathname);
      const candidate = path.resolve(root, `.${pathname}`);
      if (candidate !== root && !candidate.startsWith(root + path.sep)) {
        response.writeHead(403).end();
        return;
      }
      const file = [candidate, `${candidate}.html`, path.join(candidate, 'index.html')]
        .find(p => fs.existsSync(p) && fs.statSync(p).isFile());
      if (!file) { response.writeHead(404).end(); return; }
      response.writeHead(200, { 'Content-Type': types[path.extname(file)] || 'application/octet-stream' });
      fs.createReadStream(file).pipe(response);
    } catch {
      response.writeHead(400).end();
    }
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  server.unref();
  return server;
}

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
  await page.addInitScript(() => {
    const callbacks = new Map();
    const listeners = new Map();
    const unknownCommands = new Set();
    let nextId = 1;
    const initial = {
      ready: false,
      downloads: 0,
      cancels: 0,
      providerWrites: 0,
      download: { state: 'idle', progress: null, message: null },
    };
    const state = JSON.parse(sessionStorage.getItem('catalog-ipc') || 'null') || initial;
    const persist = () => sessionStorage.setItem('catalog-ipc', JSON.stringify(state));
    const emit = (event, payload) => {
      for (const [id, listener] of listeners) {
        if (listener.event === event) callbacks.get(listener.handler)?.({ event, id, payload });
      }
    };
    const progress = {
      phase: 'downloading', current_file: 'model.onnx', file_index: 1,
      file_count: 8, bytes_done: 25, bytes_total: 100,
    };
    const meeting = {
      id: 'catalog-smoke', title: 'Catalog smoke',
      created_at: '2026-09-10T12:00:00Z', updated_at: '2026-09-10T12:00:00Z',
      folder_path: '/fixture/meeting', duration: 10, transcript_count: 0,
      transcripts: [],
    };
    window.__catalogMock = {
      state,
      unknownCommands,
      complete() {
        state.ready = true;
        state.download = { state: 'ready', progress: null, message: null };
        persist();
        emit('gigastt-model-state', state.download);
      },
      fail() {
        state.ready = false;
        state.download = { state: 'failed', progress: null, message: 'Test download interrupted' };
        persist();
        emit('gigastt-model-state', state.download);
      },
    };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
      transformCallback(callback, once = false) {
        const id = nextId++;
        callbacks.set(id, value => { if (once) callbacks.delete(id); callback(value); });
        return id;
      },
      unregisterCallback(id) { callbacks.delete(id); },
      convertFileSrc(value) { return value; },
      async invoke(command, args = {}) {
        if (command === 'plugin:event|listen') {
          const id = nextId++;
          listeners.set(id, args);
          return id;
        }
        if (command === 'plugin:event|unlisten') { listeners.delete(args.eventId); return null; }
        if (command === 'plugin:event|emit') { emit(args.event, args.payload); return null; }
        if (command === 'plugin:os|platform') return 'windows';
        if (command === 'plugin:app|version') return '1.4.1';
        if (command === 'get_onboarding_status') return {
          completed: true, current_step: 4,
          model_status: { parakeet: 'downloaded', summary: 'downloaded', selected_summary_model: 'gemma4:e2b' },
        };
        if (command === 'check_first_launch') return false;
        if (command === 'get_recording_state') return {
          is_recording: false, is_paused: false, is_active: false,
          recording_duration: 0, active_duration: 0, mic_frames: 0,
          meeting_id: null, recording_session_id: null,
        };
        if (command === 'api_get_transcript_config') return { provider: 'local', model: 'gigaam-v3-rnnt-q8', apiKey: null };
        if (command === 'api_save_transcript_config') { state.providerWrites++; persist(); return null; }
        if (command === 'api_get_model_config') return { provider: 'builtin-ai', model: 'gemma4:e2b', whisperModel: 'large-v3' };
        if (command === 'get_recording_preferences') return {
          auto_save: true, save_folder: '/fixture/recordings', file_format: 'mp4',
          preferred_mic_device: null, preferred_system_device: null,
        };
        if (command === 'get_notification_settings') return {
          recording_notifications: false, time_based_reminders: false,
          meeting_reminders: false, respect_do_not_disturb: false,
          notification_sound: false, consent_given: false,
          manual_dnd_mode: false, system_permission_granted: false,
          notification_preferences: {
            show_recording_started: false, show_recording_stopped: false,
            show_recording_paused: false, show_recording_resumed: false,
            show_transcription_complete: false, show_meeting_reminders: false,
            show_system_errors: false, show_call_detected: false,
            meeting_reminder_minutes: [],
          },
        };
        if (command === 'gigastt_get_settings') return { auto_transcribe: true, live_preview: false };
        if (command === 'gigastt_get_job_state') return null;
        if (command === 'gigastt_model_download_state') return structuredClone(state.download);
        if (command === 'gigastt_model_status') return {
          version: '2.18.0',
          files: Array.from({ length: 8 }, (_, i) => ({ relative_path: `model-${i}.onnx`, state: state.ready ? 'valid' : 'missing' })),
        };
        if (command === 'gigastt_install_models') {
          if (state.download.state === 'running') throw new Error('Duplicate model installation');
          state.downloads++;
          state.download = { state: 'running', progress, message: null };
          persist();
          emit('gigastt-model-state', state.download);
          emit('gigastt-model-progress', progress);
          return null;
        }
        if (command === 'gigastt_cancel_model_install') {
          state.cancels++;
          state.download = { state: 'cancelled', progress: null, message: null };
          persist();
          emit('gigastt-model-state', state.download);
          return true;
        }
        if (command === 'transcribe_get_available_models') return [{
          name: 'gigaam-v3-rnnt-q8', path: '/fixture/gigaam.gguf', size_mb: 260,
          accuracy: 'Decent', wer: 8.08, wer_set: 'fixture', speed: 'Fast',
          status: 'Available', description: 'GigaAM', streaming: false,
          languages: ['ru'], recommended: false, diarizes: false,
        }];
        if (command === 'api_get_meeting' || command === 'api_get_meeting_metadata') return meeting;
        if (command === 'api_get_meeting_transcripts') return { transcripts: [], total_count: 0, has_more: false };
        if (command === 'api_get_summary') return {
          status: 'idle', meeting_name: meeting.title, meeting_id: meeting.id,
          start: null, end: null, data: null, error: null,
        };
        if (command === 'api_get_meetings') return [];
        if (command === 'api_list_templates' || command === 'get_ollama_models') return [];
        if (command === 'builtin_ai_get_recommended_model') return 'gemma4:e2b';
        if (command === 'builtin_ai_is_model_ready' || command === 'transcribe_has_available_models') return true;
        if (/get_available_models|get_audio_devices|enumerate_devices|plugin:window\|get_all/.test(command)) return [];
        if (/get_.*directory|models_directory|recordings_folder_path/.test(command)) return '/fixture';
        if (/api_get_.*api_key/.test(command)) return '';
        unknownCommands.add(command);
        return null;
      },
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener(id) { listeners.delete(id); } };
  });

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
    await meetingPanel.getByText(/Ready.*v2\.18\.0/).waitFor();
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

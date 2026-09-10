// Shared deterministic native IPC fixture for built-page browser integration tests.
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const assert = require('node:assert/strict');

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

function installTauriFixture(options = {}) {
    const callbacks = new Map();
    const listeners = new Map();
    const unknownCommands = new Set();
    let selectionCalls = 0;
    let releaseSelection;
    let nextId = 1;
    const initial = {
      ready: options.modelsReady === true,
      summaryReady: options.fresh !== true,
      liveReady: options.fresh !== true,
      initialized: options.fresh !== true,
      onboardingStatus: options.fresh ? null : {
        completed: true, current_step: 4,
        model_status: { parakeet: 'downloaded', summary: 'downloaded', selected_summary_model: 'gemma4:e2b' },
      },
      summaryDownloads: 0,
      liveDownloads: 0,
      summaryRunning: false,
      liveRunning: false,
      calls: [],
      importRequests: [],
      importFailures: options.importFailures || 0,
      job: null,
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
      releaseSelection() { releaseSelection?.(); },
      finishJob() {
        state.job = { ...state.job, state: 'ready' };
        persist();
        emit('gigastt-transcription-progress', state.job);
      },
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
    window.__TAURI_OS_PLUGIN_INTERNALS__ = { platform: options.platform || 'windows' };
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
        state.calls.push(command);
        persist();
        if (command === 'plugin:event|listen') {
          const id = nextId++;
          listeners.set(id, args);
          return id;
        }
        if (command === 'plugin:event|unlisten') { listeners.delete(args.eventId); return null; }
        if (command === 'plugin:event|emit') { emit(args.event, args.payload); return null; }
        if (command === 'plugin:os|platform') return options.platform || 'windows';
        if (command === 'plugin:app|version') return '1.4.1';
        if (command === 'get_onboarding_status') return structuredClone(state.onboardingStatus);
        if (command === 'check_first_launch') return !state.initialized;
        if (command === 'initialize_fresh_database') { state.initialized = true; persist(); return null; }
        if (command === 'save_onboarding_status_cmd') { state.onboardingStatus = args.status; persist(); return null; }
        if (command === 'complete_onboarding') {
          state.onboardingStatus = {
            completed: true, current_step: 4,
            model_status: {
              parakeet: state.liveReady ? 'downloaded' : 'not_downloaded',
              summary: state.summaryReady ? 'downloaded' : 'not_downloaded',
              selected_summary_model: args.model,
            },
          };
          persist();
          return null;
        }
        if (command === 'get_recording_state') return {
          is_recording: false, is_paused: false, is_active: false,
          recording_duration: 0, active_duration: 0, mic_frames: 0,
          meeting_id: null, recording_session_id: null,
        };
        if (command === 'api_get_transcript_config') return { provider: 'local', model: options.liveModel || 'gigaam-v3-rnnt-q8', apiKey: null };
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
        if (command === 'gigastt_get_settings') {
          if (options.settingsDelayMs) await new Promise(resolve => setTimeout(resolve, options.settingsDelayMs));
          if (options.settingsError) throw new Error('Fixture could not load transcription settings');
          return {
            auto_transcribe: options.autoTranscribe !== false,
            live_preview: options.livePreview === true,
          };
        }
        if (command === 'gigastt_get_job_state') return structuredClone(state.job);
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
          name: options.fresh ? 'parakeet-tdt-0.6b-v3-q8' : 'gigaam-v3-rnnt-q8', path: '/fixture/gigaam.gguf', size_mb: 260,
          accuracy: 'Decent', wer: 8.08, wer_set: 'fixture', speed: 'Fast',
          status: state.liveRunning ? { Downloading: { progress: 5 } } : state.liveReady ? 'Available' : 'Missing', description: 'GigaAM', streaming: false,
          languages: ['ru'], recommended: false, diarizes: false,
        }];
        if (command === 'api_get_meeting' || command === 'api_get_meeting_metadata') return { ...meeting, id: args.meetingId || meeting.id };
        if (command === 'api_get_meeting_transcripts') return {
          transcripts: state.job?.state === 'ready' ? [{ id: 'fixture-transcript', text: 'Test imported transcript', timestamp: '00:00', audio_start_time: 0, audio_end_time: 1 }] : [],
          total_count: state.job?.state === 'ready' ? 1 : 0, has_more: false,
        };
        if (command === 'api_get_summary') return {
          status: 'idle', meeting_name: meeting.title, meeting_id: meeting.id,
          start: null, end: null, data: null, error: null,
        };
        if (command === 'api_get_meetings') return state.job ? [{ ...meeting, id: state.job.meeting_id }] : [];
        if (command === 'api_list_templates' || command === 'get_ollama_models') return [];
        if (command === 'builtin_ai_get_recommended_model') return 'gemma4:e2b';
        if (command === 'builtin_ai_is_model_ready') return state.summaryReady;
        if (command === 'transcribe_has_available_models') return state.liveReady;
        if (command === 'builtin_ai_get_model_info') return { status: { type: state.summaryRunning ? 'downloading' : state.summaryReady ? 'downloaded' : 'not_downloaded' } };
        if (command === 'builtin_ai_download_model') {
          state.summaryDownloads++;
          state.summaryRunning = true;
          persist();
          emit('builtin-ai-download-progress', { model: args.modelName, progress: 5, status: 'downloading' });
          return null;
        }
        if (command === 'transcribe_download_model') {
          state.liveDownloads++;
          state.liveRunning = true;
          persist();
          emit('model-download-progress', { modelName: args.modelName, progress: 5, status: 'downloading' });
          return null;
        }
        if (command === 'select_and_validate_audio_command' || command === 'validate_audio_file_command') {
          selectionCalls++;
          const filename = options.stallFirstSelection && selectionCalls > 1 ? 'second.mp4' : 'input.mp4';
          const info = { path: args.path || `/fixture/${filename}`, filename, duration_seconds: 10, size_bytes: 4096, format: 'mp4' };
          if (options.stallFirstSelection && selectionCalls === 1) {
            return new Promise(resolve => { releaseSelection = () => resolve(info); });
          }
          return info;
        }
        if (command === 'gigastt_import_audio') {
          state.importRequests.push(args);
          if (state.importFailures > 0) {
            state.importFailures--;
            persist();
            throw new Error('Test import staging failed');
          }
          state.job = { meeting_id: 'imported-fixture', run_id: 'import-run', state: 'preparing_audio', cleanup_pending: false };
          persist();
          return structuredClone(state.job);
        }
        if (/get_available_models|get_audio_devices|enumerate_devices|plugin:window\|get_all/.test(command)) return [];
        if (/get_.*directory|models_directory|recordings_folder_path/.test(command)) return '/fixture';
        if (/api_get_.*api_key/.test(command)) return '';
        unknownCommands.add(command);
        return null;
      },
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener(id) { listeners.delete(id); } };
}

module.exports = { serveBuiltUI, installTauriFixture };

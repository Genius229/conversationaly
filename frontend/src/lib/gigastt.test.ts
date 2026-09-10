const assert = require('node:assert/strict');
const test = require('node:test');
const {
  canAutomaticallyPostProcess,
  canAutomaticallyGenerateSummary,
  canSummarizeCurrentDraft,
  isGigasttJobActive,
  isGigasttModelReady,
  recordingSaveDescription,
  mergeGigasttJobEvent,
  modelInstallPercent,
  requiresLiveTranscriptionModel,
} = require('./gigastt.ts') as typeof import('./gigastt');

type GigasttJobSnapshot = import('./gigastt').GigasttJobSnapshot;

const job = (
  state: GigasttJobSnapshot['state'],
  overrides: Partial<GigasttJobSnapshot> = {},
): GigasttJobSnapshot => ({
  meeting_id: 'meeting-1',
  run_id: 'run-1',
  state,
  cleanup_pending: false,
  ...overrides,
});

test('live model is bypassed only when loaded settings explicitly disable preview', () => {
  assert.equal(requiresLiveTranscriptionModel({ auto_transcribe: true, live_preview: false }), false);
  assert.equal(requiresLiveTranscriptionModel({ auto_transcribe: true, live_preview: true }), true);
  assert.equal(requiresLiveTranscriptionModel(null), true);
});

test('automatic post-processing waits for a final transcript and never uses failed draft text', () => {
  assert.equal(canAutomaticallyPostProcess(true, undefined), false);
  assert.equal(canAutomaticallyPostProcess(true, job('transcribing')), false);
  assert.equal(canAutomaticallyPostProcess(true, job('failed')), false);
  assert.equal(canAutomaticallyPostProcess(true, job('cancelled')), false);
  assert.equal(canAutomaticallyPostProcess(true, job('ready')), false);
  assert.equal(canAutomaticallyPostProcess(true, job('ready'), true), true);
  assert.equal(canAutomaticallyPostProcess(true, null), true);
  assert.equal(canAutomaticallyPostProcess(false, job('ready'), true), false);
});

test('automatic summary waits for optional speaker labelling to settle', () => {
  assert.equal(canAutomaticallyGenerateSummary(true, false, false), true);
  assert.equal(canAutomaticallyGenerateSummary(true, true, false), false);
  assert.equal(canAutomaticallyGenerateSummary(true, true, true), true);
  assert.equal(canAutomaticallyGenerateSummary(false, true, true), false);
});

test('only a failed or cancelled finalization exposes explicit draft summary', () => {
  assert.equal(canSummarizeCurrentDraft(job('failed')), true);
  assert.equal(canSummarizeCurrentDraft(job('cancelled')), true);
  assert.equal(canSummarizeCurrentDraft(job('ready')), false);
  assert.equal(canSummarizeCurrentDraft(null), false);
});

test('active job states include preparation through finalization only', () => {
  for (const state of ['preparing_audio', 'starting', 'transcribing', 'finalizing'] as const) {
    assert.equal(isGigasttJobActive(job(state)), true);
  }
  assert.equal(isGigasttJobActive(job('ready')), false);
  assert.equal(isGigasttJobActive(job('failed')), false);
});

test('job event merge filters other meetings and stale run ids', () => {
  const current = job('transcribing', { percent: 40 });
  assert.equal(
    mergeGigasttJobEvent(current, job('ready', { meeting_id: 'meeting-2' }), 'meeting-1'),
    current,
  );
  assert.equal(
    mergeGigasttJobEvent(current, job('failed', { run_id: 'old-run' }), 'meeting-1'),
    current,
  );
  assert.deepEqual(
    mergeGigasttJobEvent(current, job('ready'), 'meeting-1'),
    job('ready'),
  );
  assert.deepEqual(
    mergeGigasttJobEvent(null, job('preparing_audio'), 'meeting-1'),
    job('preparing_audio'),
  );
});

test('model readiness requires every pinned file to be valid', () => {
  assert.equal(isGigasttModelReady({
    version: '2.18.0',
    files: [
      { relative_path: 'encoder.onnx', state: 'valid' },
      { relative_path: 'vocab.txt', state: 'valid' },
    ],
  }), true);
  assert.equal(isGigasttModelReady({
    version: '2.18.0',
    files: [
      { relative_path: 'encoder.onnx', state: 'valid' },
      { relative_path: 'vocab.txt', state: 'missing' },
    ],
  }), false);
  assert.equal(isGigasttModelReady({
    version: '2.18.0',
    files: [
      { relative_path: 'encoder.onnx', state: 'corrupt', actual_sha256: 'bad-hash' },
    ],
  }), false);
  assert.equal(isGigasttModelReady({ version: '2.18.0', files: [] }), false);
});

test('download progress prefers bytes and falls back to completed files', () => {
  assert.equal(modelInstallPercent({
    phase: 'downloading',
    current_file: 'encoder.onnx',
    file_index: 1,
    file_count: 4,
    bytes_done: 25,
    bytes_total: 100,
  }), 6.25);
  assert.equal(modelInstallPercent({
    phase: 'verifying',
    current_file: 'vocab.txt',
    file_index: 3,
    file_count: 4,
    bytes_done: 0,
    bytes_total: null,
  }), 75);
  assert.equal(modelInstallPercent({
    phase: 'checking',
    current_file: 'encoder.onnx',
    file_index: 1,
    file_count: 8,
    bytes_done: 0,
    bytes_total: null,
  }), 0);
  assert.equal(modelInstallPercent({
    phase: 'downloading',
    current_file: 'decoder.onnx',
    file_index: 2,
    file_count: 8,
    bytes_done: 0,
    bytes_total: null,
  }), 12.5);
  assert.equal(modelInstallPercent({
    phase: 'reused',
    current_file: 'encoder.onnx',
    file_index: 1,
    file_count: 8,
    bytes_done: 0,
    bytes_total: null,
  }), 12.5);
});

test('recording save copy reflects the actual finalization outcome', () => {
  assert.equal(
    recordingSaveDescription('accepted', 0),
    'Audio saved. Final GigaSTT transcription is continuing in the meeting view.',
  );
  assert.equal(
    recordingSaveDescription('disabled', 0),
    'Audio saved. Automatic final transcription is off.',
  );
  assert.equal(
    recordingSaveDescription('disabled', 3),
    '3 draft transcript segments saved. Automatic final transcription is off.',
  );
  assert.equal(
    recordingSaveDescription('failed', 2),
    'Audio and 2 draft transcript segments saved. Open the meeting to retry GigaSTT.',
  );
});

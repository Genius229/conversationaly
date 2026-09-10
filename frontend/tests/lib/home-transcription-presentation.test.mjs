import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const read = relativePath => {
  const file = path.join(root, relativePath);
  return fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : '';
};

test('home reads the persisted GigaSTT mode and cleans up settings subscriptions', () => {
  const hook = read('src/hooks/useGigasttSettings.ts');
  const service = read('src/services/gigasttService.ts');

  assert.match(hook, /gigasttService\.getSettings\(\)/);
  assert.match(hook, /gigasttService\.subscribeSettings/);
  assert.match(hook, /unsubscribe\(\)/);
  assert.match(service, /settingsChannel\.publish\(settings\)/);
  assert.match(service, /subscribeSettings/);
});

test('home header and empty state use the settings-derived presentation', () => {
  const panel = read('src/app/_components/TranscriptPanel.tsx');
  const transcriptView = read('src/components/VirtualizedTranscriptView.tsx');

  assert.match(panel, /homeTranscriptionPresentation/);
  assert.match(panel, /useGigasttSettings/);
  assert.match(panel, /emptyStateCopy=\{presentation\}/);
  assert.doesNotMatch(panel, /transcriptModelConfig\.model \|\| 'No model selected'/);
  assert.match(transcriptView, /emptyStateCopy\?: HomeTranscriptionPresentation/);
});

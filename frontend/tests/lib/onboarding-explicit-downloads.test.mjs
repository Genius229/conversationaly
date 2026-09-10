import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const contextSource = fs.readFileSync(path.join(root, 'src/contexts/OnboardingContext.tsx'), 'utf8');
const stepSource = fs.readFileSync(path.join(root, 'src/components/onboarding/steps/DownloadProgressStep.tsx'), 'utf8');

function parse(source) {
  return ts.createSourceFile('source.tsx', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
}

function namedArrow(source, name) {
  const file = parse(source);
  let found;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(file) === name) found = node.initializer.getText(file);
    ts.forEachChild(node, visit);
  }
  visit(file);
  assert.ok(found, `Missing production function ${name}`);
  return found;
}

async function runCompletion(throwReadiness = false) {
  const calls = [];
  const state = {};
  const module = { exports: {} };
  const compiled = ts.transpileModule(`module.exports = ${namedArrow(contextSource, 'completeOnboarding')}`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  vm.runInNewContext(compiled, {
    module, console, clearTimeout,
    isCompletingRef: { current: false }, saveTimeoutRef: { current: undefined },
    selectedSummaryModel: 'gemma4:e2b',
    setSelectedSummaryModel: value => { state.model = value; },
    setSummaryModelDownloaded: value => { state.downloaded = value; },
    setCurrentStep: value => { state.step = value; },
    setCompleted: value => { state.completed = value; },
    invoke: async (command, args) => {
      calls.push([command, args]);
      if (command === 'builtin_ai_is_model_ready') {
        if (throwReadiness) throw new Error('Readiness unavailable');
        return false;
      }
      if (command === 'builtin_ai_get_recommended_model') return 'gemma4:e2b';
      return null;
    },
    requestSummaryModelDownload: modelName => calls.push(['builtin_ai_download_model', { modelName }]),
  });
  await module.exports();
  return { calls, state };
}

test('completing setup with missing models never starts a model download', async () => {
  const { calls, state } = await runCompletion();
  assert.equal(state.completed, true);
  assert.equal(state.downloaded, false);
  assert.ok(calls.some(([command]) => command === 'complete_onboarding'));
  assert.equal(calls.filter(([command]) => command.includes('download_model')).length, 0);
});

test('readiness inspection failure does not prevent model-free completion', async () => {
  const { calls, state } = await runCompletion(true);
  assert.equal(state.completed, true);
  assert.equal(state.downloaded, false);
  assert.ok(calls.some(([command]) => command === 'complete_onboarding'));
});

test('mounting or updating the model step cannot start downloads', () => {
  const file = parse(stepSource);
  const automatic = [];
  function visit(node) {
    if (ts.isCallExpression(node) && node.expression.getText(file) === 'useEffect') {
      const callback = node.arguments[0];
      function inspect(child) {
        if (ts.isCallExpression(child)) {
          const call = child.expression.getText(file);
          if (['startBackgroundDownloads', 'startSummaryDownload', 'handleRetryDownload', 'handleRetrySummaryDownload'].includes(call)) automatic.push(call);
          if (call === 'invoke' && /download_model/.test(child.arguments[0]?.getText(file) || '')) automatic.push(call);
        }
        ts.forEachChild(child, inspect);
      }
      if (callback) inspect(callback);
    }
    ts.forEachChild(node, visit);
  }
  visit(file);
  assert.deepEqual(automatic, []);
});

test('model setup exposes a clear skip and explicit initial download actions', () => {
  assert.ok(stepSource.includes('Set up models later'));
  assert.ok(stepSource.includes('Continue downloads in background'));
  assert.ok(stepSource.includes('Download summary model'));
  assert.ok(stepSource.includes('Download live model'));
});

test('byte progress alone never marks a model as downloaded', () => {
  for (const source of [contextSource, stepSource]) {
    assert.ok(!/status\s*===\s*'completed'\s*\|\|\s*progress\s*>=\s*100/.test(source));
  }
});

test('skip waits for platform and restored setup state, not model downloads', () => {
  assert.ok(stepSource.includes('isMac === null'));
  assert.ok(stepSource.includes('!modelSetupLoaded'));
});

async function restoreActiveDownloads(failLive = false) {
  const calls = [];
  let active = false;
  const module = { exports: {} };
  const compiled = ts.transpileModule(`module.exports = ${namedArrow(contextSource, 'checkActiveDownloads')}`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  vm.runInNewContext(compiled, {
    module, console, selectedSummaryModel: 'gemma4:e2b', DEFAULT_TRANSCRIBE_MODEL: 'parakeet',
    modelEventsRevisionRef: { current: 0 },
    setIsBackgroundDownloading: value => { active = value; },
    setActiveSetupDownloads: () => {},
    invoke: async command => {
      calls.push(command);
      if (failLive && command === 'transcribe_get_available_models') throw new Error('Live status unavailable');
      return command === 'transcribe_get_available_models' ? [] : { status: { type: 'downloading', progress: 20 } };
    },
  });
  await module.exports('gemma4:e2b');
  return { active, calls };
}

test('restoring an active summary download is read-only and reports its activity', async () => {
  const { active, calls } = await restoreActiveDownloads();
  assert.equal(active, true);
  assert.deepEqual(calls.filter(command => command.includes('download_model')), []);
});

test('one failed status query does not hide another active download', async () => {
  const { active } = await restoreActiveDownloads(true);
  assert.equal(active, true);
});

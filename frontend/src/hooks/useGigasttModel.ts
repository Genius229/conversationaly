'use client';

import { useSyncExternalStore } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import {
  createGigasttModelInstallAuthority,
  type GigasttModelAvailability,
  type GigasttModelDownloadState,
  type GigasttModelInstallProgress,
} from '@/lib/gigastt';
import { gigasttService } from '@/services/gigasttService';

const DEFAULT_DOWNLOAD_STATE: GigasttModelDownloadState = {
  state: 'idle',
  progress: null,
  message: null,
};

interface GigasttModelSnapshot {
  modelStatus: GigasttModelAvailability | null;
  modelDownload: GigasttModelDownloadState;
  isActing: boolean;
  error: string | null;
}

export interface UseGigasttModelResult extends GigasttModelSnapshot {
  installModels: () => Promise<void>;
  cancelModelInstall: () => Promise<void>;
  refreshModelStatus: () => Promise<void>;
}

interface ListenerSession {
  cancelled: boolean;
  unlisteners: UnlistenFn[];
}

const SERVER_SNAPSHOT: GigasttModelSnapshot = {
  modelStatus: null,
  modelDownload: DEFAULT_DOWNLOAD_STATE,
  isActing: false,
  error: null,
};

let snapshot = SERVER_SNAPSHOT;
let listenerSession: ListenerSession | null = null;
let stopTimer: ReturnType<typeof setTimeout> | null = null;
let modelStateGeneration = 0;
let actionGeneration = 0;
const subscribers = new Set<() => void>();
const installAuthority = createGigasttModelInstallAuthority();

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function publish(patch: Partial<GigasttModelSnapshot>) {
  snapshot = { ...snapshot, ...patch };
  subscribers.forEach(notify => notify());
}

function getSnapshot(): GigasttModelSnapshot {
  return snapshot;
}

function getServerSnapshot(): GigasttModelSnapshot {
  return SERVER_SNAPSHOT;
}

async function refreshModelStatus(): Promise<void> {
  const generation = modelStateGeneration;
  try {
    const status = await gigasttService.getModelStatus();
    if (modelStateGeneration === generation) {
      publish({ modelStatus: status, error: null });
    }
  } catch (error) {
    if (modelStateGeneration === generation) {
      publish({ error: `Could not check GigaSTT models: ${errorMessage(error)}` });
    }
  }
}

async function startListeners(session: ListenerSession) {
  const listenerResults = await Promise.allSettled([
    listen<GigasttModelInstallProgress>('gigastt-model-progress', event => {
      if (session.cancelled || listenerSession !== session) return;
      modelStateGeneration += 1;
      publish({
        modelStatus: null,
        modelDownload: { state: 'running', progress: event.payload, message: null },
        error: null,
      });
    }),
    listen<GigasttModelDownloadState>('gigastt-model-state', event => {
      if (session.cancelled || listenerSession !== session) return;
      modelStateGeneration += 1;
      const terminal = event.payload.state !== 'running' && event.payload.state !== 'idle';
      publish({
        modelDownload: event.payload,
        modelStatus: terminal ? null : snapshot.modelStatus,
        error: null,
      });
      if (terminal) void refreshModelStatus();
    }),
  ]);

  const listenerErrors: string[] = [];
  for (const result of listenerResults) {
    if (result.status === 'fulfilled') {
      if (session.cancelled || listenerSession !== session) result.value();
      else session.unlisteners.push(result.value);
    } else {
      listenerErrors.push(errorMessage(result.reason));
    }
  }

  if (session.cancelled || listenerSession !== session) return;
  if (listenerErrors.length > 0) {
    session.unlisteners.forEach(unlisten => unlisten());
    session.unlisteners = [];
    publish({ error: `Could not listen for GigaSTT model progress: ${listenerErrors.join('; ')}` });
  }

  const generation = modelStateGeneration;
  const [downloadState, availability] = await Promise.allSettled([
    gigasttService.getModelDownloadState(),
    gigasttService.getModelStatus(),
  ]);
  if (session.cancelled || listenerSession !== session) return;

  const loadErrors: string[] = [];
  const patch: Partial<GigasttModelSnapshot> = {};
  if (modelStateGeneration === generation) {
    if (downloadState.status === 'fulfilled') patch.modelDownload = downloadState.value;
    else loadErrors.push(`download state: ${errorMessage(downloadState.reason)}`);

    if (availability.status === 'fulfilled') patch.modelStatus = availability.value;
    else loadErrors.push(`model files: ${errorMessage(availability.reason)}`);
  }
  if (loadErrors.length > 0) {
    patch.error = `Could not load GigaSTT ${loadErrors.join('; ')}`;
  } else if (listenerErrors.length === 0) {
    patch.error = null;
  }
  if (Object.keys(patch).length > 0) publish(patch);
}

function ensureListeners() {
  if (listenerSession) return;
  const session: ListenerSession = { cancelled: false, unlisteners: [] };
  listenerSession = session;
  void startListeners(session);
}

function stopListeners() {
  const session = listenerSession;
  listenerSession = null;
  if (!session) return;
  session.cancelled = true;
  session.unlisteners.forEach(unlisten => unlisten());
  session.unlisteners = [];
}

function subscribe(notify: () => void): () => void {
  if (stopTimer) {
    clearTimeout(stopTimer);
    stopTimer = null;
  }
  subscribers.add(notify);
  ensureListeners();

  return () => {
    subscribers.delete(notify);
    if (subscribers.size === 0 && !stopTimer) {
      // React Strict Mode briefly unsubscribes and resubscribes effects in
      // development. Defer teardown one task so that probe does not duplicate
      // native listeners while still releasing them when the last UI leaves.
      stopTimer = setTimeout(() => {
        stopTimer = null;
        if (subscribers.size === 0) stopListeners();
      }, 0);
    }
  };
}

async function installModels(): Promise<void> {
  if (snapshot.modelDownload.state === 'running') return;
  return installAuthority.run(async () => {
    const generation = ++actionGeneration;
    publish({ isActing: true, error: null });
    try {
      await gigasttService.installModels();
      const modelGeneration = modelStateGeneration;
      const state = await gigasttService.getModelDownloadState();
      if (modelStateGeneration === modelGeneration) publish({ modelDownload: state });
    } catch (error) {
      if (actionGeneration === generation) publish({ error: errorMessage(error) });
    } finally {
      if (actionGeneration === generation) publish({ isActing: false });
    }
  });
}

async function cancelModelInstall(): Promise<void> {
  const generation = ++actionGeneration;
  publish({ isActing: true, error: null });
  try {
    const accepted = await gigasttService.cancelModelInstall();
    if (!accepted) throw new Error('The GigaSTT model download is no longer running.');
  } catch (error) {
    if (actionGeneration === generation) publish({ error: errorMessage(error) });
  } finally {
    if (actionGeneration === generation) publish({ isActing: false });
  }
}

export function useGigasttModel(): UseGigasttModelResult {
  const state = useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);
  return {
    ...state,
    installModels,
    cancelModelInstall,
    refreshModelStatus,
  };
}

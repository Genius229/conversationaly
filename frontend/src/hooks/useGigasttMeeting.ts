import { useCallback, useEffect, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import {
  mergeGigasttJobEvent,
  type GigasttJobSnapshot,
  type GigasttModelAvailability,
  type GigasttModelDownloadState,
  type GigasttModelInstallProgress,
  type GigasttSettings,
} from '@/lib/gigastt';
import { gigasttService } from '@/services/gigasttService';

const DEFAULT_DOWNLOAD_STATE: GigasttModelDownloadState = {
  state: 'idle',
  progress: null,
  message: null,
};

export interface UseGigasttMeetingResult {
  job: GigasttJobSnapshot | null | undefined;
  transcriptReady: boolean;
  isRefreshingTranscript: boolean;
  settings: GigasttSettings | null;
  modelStatus: GigasttModelAvailability | null;
  modelDownload: GigasttModelDownloadState;
  isActing: boolean;
  error: string | null;
  startTranscription: () => Promise<void>;
  cancelTranscription: () => Promise<void>;
  installModels: () => Promise<void>;
  cancelModelInstall: () => Promise<void>;
  saveSettings: (settings: GigasttSettings) => Promise<void>;
  refreshModelStatus: () => Promise<void>;
  retryTranscriptRefresh: () => Promise<void>;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

interface LegacyRetranscriptionComplete {
  meeting_id: string;
}

export function useGigasttMeeting(
  meetingId: string,
  onTranscriptReady?: () => Promise<void>,
): UseGigasttMeetingResult {
  const [job, setJob] = useState<GigasttJobSnapshot | null | undefined>(undefined);
  const [transcriptReadyRun, setTranscriptReadyRun] = useState<string | null>(null);
  const [isRefreshingTranscript, setIsRefreshingTranscript] = useState(false);
  const [settings, setSettings] = useState<GigasttSettings | null>(null);
  const [modelStatus, setModelStatus] = useState<GigasttModelAvailability | null>(null);
  const [modelDownload, setModelDownload] = useState(DEFAULT_DOWNLOAD_STATE);
  const [isActing, setIsActing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const onTranscriptReadyRef = useRef(onTranscriptReady);
  onTranscriptReadyRef.current = onTranscriptReady;
  const activeRunRef = useRef<string | null>(null);
  const initializedRef = useRef(false);
  const pendingEventRef = useRef<GigasttJobSnapshot | null>(null);
  const commandStartingRef = useRef(false);
  const commandPreviousRunRef = useRef<string | null>(null);
  const pendingCommandEventRef = useRef<GigasttJobSnapshot | null>(null);
  const refreshedRunsRef = useRef(new Set<string>());
  const refreshingRunRef = useRef<string | null>(null);
  const meetingIdRef = useRef(meetingId);
  meetingIdRef.current = meetingId;
  const actionGenerationRef = useRef(0);
  const authorityRefreshGenerationRef = useRef(0);
  const modelStateGenerationRef = useRef(0);

  const refreshModelStatus = useCallback(async () => {
    const generation = modelStateGenerationRef.current;
    try {
      const status = await gigasttService.getModelStatus();
      if (modelStateGenerationRef.current === generation) setModelStatus(status);
    } catch (loadError) {
      if (modelStateGenerationRef.current === generation) {
        setError(`Could not check GigaSTT models: ${errorMessage(loadError)}`);
      }
    }
  }, []);

  useEffect(() => {
    if (!meetingId) return;

    let cancelled = false;
    let unlisteners: UnlistenFn[] = [];
    initializedRef.current = false;
    pendingEventRef.current = null;
    commandStartingRef.current = false;
    commandPreviousRunRef.current = null;
    pendingCommandEventRef.current = null;
    activeRunRef.current = null;
    refreshedRunsRef.current = new Set();
    refreshingRunRef.current = null;
    actionGenerationRef.current += 1;
    authorityRefreshGenerationRef.current += 1;
    modelStateGenerationRef.current += 1;
    setJob(undefined);
    setTranscriptReadyRun(null);
    setIsRefreshingTranscript(false);
    setIsActing(false);
    setError(null);

    const handleJobEvent = (incoming: GigasttJobSnapshot) => {
      if (incoming.meeting_id !== meetingId) return;
      if (commandStartingRef.current) {
        if (incoming.run_id !== commandPreviousRunRef.current) {
          pendingCommandEventRef.current = incoming;
        }
        return;
      }
      if (!initializedRef.current) {
        pendingEventRef.current = incoming;
        return;
      }
      if (activeRunRef.current && incoming.run_id !== activeRunRef.current) return;
      activeRunRef.current = incoming.run_id;
      setJob(current => mergeGigasttJobEvent(current ?? null, incoming, meetingId));
    };

    const handleLegacyRetranscriptionComplete = async (payload: LegacyRetranscriptionComplete) => {
      if (payload.meeting_id !== meetingId) return;

      // The successful alternative-provider transaction clears GigaSTT
      // authority. Remove the old badge immediately, then reload both the
      // durable authority state and replacement transcript before rendering a
      // settled state for this route.
      authorityRefreshGenerationRef.current += 1;
      activeRunRef.current = null;
      refreshedRunsRef.current = new Set();
      refreshingRunRef.current = null;
      setJob(undefined);
      setTranscriptReadyRun(null);
      setIsRefreshingTranscript(true);
      setError(null);

      const [transcriptResult, jobResult] = await Promise.allSettled([
        onTranscriptReadyRef.current?.(),
        gigasttService.getJobState(meetingId),
      ]);
      if (cancelled || meetingIdRef.current !== meetingId) return;

      if (transcriptResult.status === 'fulfilled' && jobResult.status === 'fulfilled') {
        activeRunRef.current = jobResult.value?.run_id ?? null;
        setJob(jobResult.value);
      } else {
        const failures = [
          transcriptResult.status === 'rejected'
            ? `transcript: ${errorMessage(transcriptResult.reason)}`
            : null,
          jobResult.status === 'rejected'
            ? `job state: ${errorMessage(jobResult.reason)}`
            : null,
        ].filter((value): value is string => value !== null);
        setError(`Alternative retranscription saved, but refresh failed (${failures.join('; ')})`);
      }
      setIsRefreshingTranscript(false);
    };

    const setup = async () => {
      try {
        const jobUnlisten = await listen<GigasttJobSnapshot>(
          'gigastt-transcription-progress',
          event => handleJobEvent(event.payload),
        );
        if (cancelled) {
          jobUnlisten();
          return;
        }
        unlisteners.push(jobUnlisten);

        const progressUnlisten = await listen<GigasttModelInstallProgress>(
          'gigastt-model-progress',
          event => {
            modelStateGenerationRef.current += 1;
            setModelStatus(null);
            setModelDownload({ state: 'running', progress: event.payload, message: null });
          },
        );
        if (cancelled) {
          progressUnlisten();
          return;
        }
        unlisteners.push(progressUnlisten);

        const modelStateUnlisten = await listen<GigasttModelDownloadState>(
          'gigastt-model-state',
          event => {
            modelStateGenerationRef.current += 1;
            setModelDownload(event.payload);
            if (event.payload.state !== 'running' && event.payload.state !== 'idle') {
              setModelStatus(null);
              void refreshModelStatus();
            }
          },
        );
        if (cancelled) {
          modelStateUnlisten();
          return;
        }
        unlisteners.push(modelStateUnlisten);

        const legacyRetranscriptionUnlisten = await listen<LegacyRetranscriptionComplete>(
          'retranscription-complete',
          event => void handleLegacyRetranscriptionComplete(event.payload),
        );
        if (cancelled) {
          legacyRetranscriptionUnlisten();
          return;
        }
        unlisteners.push(legacyRetranscriptionUnlisten);
      } catch (listenerError) {
        unlisteners.forEach(unlisten => unlisten());
        unlisteners = [];
        if (!cancelled) {
          setError(`Could not listen for GigaSTT progress: ${errorMessage(listenerError)}`);
        }
      }

      if (cancelled) return;
      const authorityGeneration = authorityRefreshGenerationRef.current;
      const modelGeneration = modelStateGenerationRef.current;
      const [persistedJob, loadedSettings, downloadState, availability] = await Promise.allSettled([
        gigasttService.getJobState(meetingId),
        gigasttService.getSettings(),
        gigasttService.getModelDownloadState(),
        gigasttService.getModelStatus(),
      ]);
      if (cancelled) return;

      const loadErrors: string[] = [];
      if (loadedSettings.status === 'fulfilled') setSettings(loadedSettings.value);
      else loadErrors.push(`settings: ${errorMessage(loadedSettings.reason)}`);
      if (downloadState.status === 'fulfilled') {
        if (modelStateGenerationRef.current === modelGeneration) {
          setModelDownload(downloadState.value);
        }
      } else if (modelStateGenerationRef.current === modelGeneration) {
        loadErrors.push(`download state: ${errorMessage(downloadState.reason)}`);
      }
      if (availability.status === 'fulfilled') {
        if (modelStateGenerationRef.current === modelGeneration) {
          setModelStatus(availability.value);
        }
      } else if (modelStateGenerationRef.current === modelGeneration) {
        loadErrors.push(`model files: ${errorMessage(availability.reason)}`);
      }

      if (
        persistedJob.status === 'fulfilled'
        && authorityRefreshGenerationRef.current === authorityGeneration
      ) {
        const pending = pendingEventRef.current;
        const initial = pending && (
          persistedJob.value === null || pending.run_id === persistedJob.value.run_id
        ) ? pending : persistedJob.value;
        activeRunRef.current = initial?.run_id ?? null;
        setJob(initial);
      } else if (
        persistedJob.status === 'rejected'
        && authorityRefreshGenerationRef.current === authorityGeneration
      ) {
        loadErrors.push(`job state: ${errorMessage(persistedJob.reason)}`);
      }
      if (loadErrors.length > 0) {
        setError(current => current ?? `Could not load GigaSTT ${loadErrors.join('; ')}`);
      }
      initializedRef.current = true;
      pendingEventRef.current = null;
    };

    void setup();
    return () => {
      cancelled = true;
      unlisteners.forEach(unlisten => unlisten());
    };
  }, [meetingId, refreshModelStatus]);

  const retryTranscriptRefresh = useCallback(async () => {
    if (job?.state !== 'ready') return;
    const runId = job.run_id;
    if (refreshedRunsRef.current.has(runId) || refreshingRunRef.current === runId) return;

    refreshingRunRef.current = runId;
    setIsRefreshingTranscript(true);
    setError(null);
    try {
      await onTranscriptReadyRef.current?.();
      if (meetingIdRef.current === meetingId && activeRunRef.current === runId) {
        refreshedRunsRef.current.add(runId);
        setTranscriptReadyRun(runId);
      }
    } catch (refreshError) {
      if (meetingIdRef.current === meetingId && activeRunRef.current === runId) {
        setError(`Final transcript is ready but could not be refreshed: ${errorMessage(refreshError)}`);
      }
    } finally {
      if (refreshingRunRef.current === runId) {
        refreshingRunRef.current = null;
        setIsRefreshingTranscript(false);
      }
    }
  }, [job, meetingId]);

  useEffect(() => {
    if (job?.state === 'ready') void retryTranscriptRefresh();
  }, [job?.state, job?.run_id, retryTranscriptRefresh]);

  const runAction = useCallback(async (action: () => Promise<void>) => {
    const generation = ++actionGenerationRef.current;
    setIsActing(true);
    setError(null);
    try {
      await action();
    } catch (actionError) {
      if (actionGenerationRef.current === generation) setError(errorMessage(actionError));
    } finally {
      if (actionGenerationRef.current === generation) setIsActing(false);
    }
  }, []);

  const startTranscription = useCallback(async () => {
    await runAction(async () => {
      const previousRun = activeRunRef.current;
      const previousJob = job;
      let installedNewRun = false;
      commandPreviousRunRef.current = previousRun;
      pendingCommandEventRef.current = null;
      commandStartingRef.current = true;
      // Gate automatic post-processing synchronously, before the native invoke
      // can wait on the previous worker or emit the new run's first event.
      refreshingRunRef.current = null;
      setJob(undefined);
      setTranscriptReadyRun(null);
      setIsRefreshingTranscript(false);
      try {
        const started = await gigasttService.transcribeMeeting(meetingId);
        if (meetingIdRef.current !== meetingId) return;
        // React event callbacks may update the ref while the invoke is pending;
        // TypeScript cannot observe that cross-callback mutation.
        const pending = pendingCommandEventRef.current as GigasttJobSnapshot | null;
        activeRunRef.current = started.run_id;
        setJob(pending?.run_id === started.run_id ? pending : started);
        installedNewRun = true;
      } finally {
        commandStartingRef.current = false;
        commandPreviousRunRef.current = null;
        pendingCommandEventRef.current = null;
        if (meetingIdRef.current === meetingId && activeRunRef.current === null) {
          activeRunRef.current = previousRun;
        }
        if (meetingIdRef.current === meetingId && !installedNewRun) {
          setJob(previousJob);
        }
      }
    });
  }, [job, meetingId, runAction]);

  const cancelTranscription = useCallback(async () => {
    await runAction(async () => {
      const accepted = await gigasttService.cancelTranscription(meetingId);
      if (!accepted) throw new Error('The GigaSTT job is no longer running.');
    });
  }, [meetingId, runAction]);

  const installModels = useCallback(async () => {
    await runAction(async () => {
      await gigasttService.installModels();
      const generation = modelStateGenerationRef.current;
      const state = await gigasttService.getModelDownloadState();
      if (modelStateGenerationRef.current === generation) setModelDownload(state);
    });
  }, [runAction]);

  const cancelModelInstall = useCallback(async () => {
    await runAction(async () => {
      const accepted = await gigasttService.cancelModelInstall();
      if (!accepted) throw new Error('The GigaSTT model download is no longer running.');
    });
  }, [runAction]);

  const saveSettings = useCallback(async (nextSettings: GigasttSettings) => {
    const previous = settings;
    setIsActing(true);
    setSettings(nextSettings);
    setError(null);
    try {
      await gigasttService.saveSettings(nextSettings);
    } catch (saveError) {
      setSettings(previous);
      setError(`Could not save GigaSTT settings: ${errorMessage(saveError)}`);
    } finally {
      setIsActing(false);
    }
  }, [settings]);

  return {
    job,
    transcriptReady: job?.state === 'ready' && transcriptReadyRun === job.run_id,
    isRefreshingTranscript,
    settings,
    modelStatus,
    modelDownload,
    isActing,
    error,
    startTranscription,
    cancelTranscription,
    installModels,
    cancelModelInstall,
    saveSettings,
    refreshModelStatus,
    retryTranscriptRefresh,
  };
}

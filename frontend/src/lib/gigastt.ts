export interface GigasttSettings {
  auto_transcribe: boolean;
  live_preview: boolean;
}

export type GigasttJobState =
  | 'preparing_audio'
  | 'starting'
  | 'transcribing'
  | 'finalizing'
  | 'ready'
  | 'failed'
  | 'cancelled';

export interface GigasttJobSnapshot {
  meeting_id: string;
  run_id: string;
  state: GigasttJobState;
  percent?: number;
  message?: string;
  cleanup_pending: boolean;
}

export type GigasttModelFileState = 'valid' | 'missing' | 'corrupt';

export interface GigasttModelAvailability {
  version: string;
  files: Array<{
    relative_path: string;
    state: GigasttModelFileState;
    actual_sha256?: string;
  }>;
}

export interface GigasttModelInstallProgress {
  phase: string;
  current_file: string | null;
  file_index: number;
  file_count: number;
  bytes_done: number;
  bytes_total: number | null;
}

export type GigasttModelDownloadStatus =
  | 'idle'
  | 'running'
  | 'ready'
  | 'failed'
  | 'cancelled';

export interface GigasttModelDownloadState {
  state: GigasttModelDownloadStatus;
  progress: GigasttModelInstallProgress | null;
  message: string | null;
}

export const GIGASTT_CATALOG_MODEL = {
  id: 'gigastt-post-recording',
  label: 'GigaSTT',
  version: '2.18.0',
  useTag: 'After recording · Offline',
  description:
    'Creates the final Russian transcript after recording. Separate from the live model selected for recording.',
  fileCount: 8,
} as const;

export interface GigasttCatalogView {
  installed: boolean;
  ready: boolean;
  statusLabel: string;
  actionLabel: 'Download model' | 'Repair model' | null;
  filesLabel: string;
}

const GIGASTT_CATALOG_SEARCH_TEXT = [
  GIGASTT_CATALOG_MODEL.label,
  GIGASTT_CATALOG_MODEL.version,
  GIGASTT_CATALOG_MODEL.useTag,
  GIGASTT_CATALOG_MODEL.description,
  'Russian final transcript',
].join(' ').toLowerCase();

export function gigasttCatalogMatches(query: string): boolean {
  const normalized = query.trim().toLowerCase();
  return normalized.length === 0 || GIGASTT_CATALOG_SEARCH_TEXT.includes(normalized);
}

export function gigasttCatalogView(
  status: GigasttModelAvailability | null,
  download: GigasttModelDownloadState,
): GigasttCatalogView {
  if (download.state === 'running') {
    const phase = download.progress?.phase ?? 'downloading';
    return {
      installed: true,
      ready: false,
      statusLabel: `${phase.charAt(0).toUpperCase()}${phase.slice(1)} · ${Math.round(modelInstallPercent(download.progress))}%`,
      actionLabel: null,
      filesLabel: `${GIGASTT_CATALOG_MODEL.fileCount} required`,
    };
  }

  if (status === null) {
    const installCompleted = download.state === 'ready';
    return {
      installed: installCompleted,
      ready: false,
      statusLabel: installCompleted ? 'Verifying local files' : 'Checking local files',
      actionLabel: null,
      filesLabel: `${GIGASTT_CATALOG_MODEL.fileCount} required`,
    };
  }

  const ready = isGigasttModelReady(status);
  if (ready) {
    return {
      installed: true,
      ready: true,
      statusLabel: `Ready · v${status.version}`,
      actionLabel: null,
      filesLabel: `${GIGASTT_CATALOG_MODEL.fileCount} verified`,
    };
  }

  const missing = status.files.filter(file => file.state === 'missing').length;
  const damaged = status.files.filter(file => file.state === 'corrupt').length;
  const installed = status.files.some(file => file.state !== 'missing');
  const problems = missing + damaged;
  const statusLabel = missing > 0 && damaged > 0
    ? `${problems} files missing or damaged`
    : damaged > 0
      ? `${damaged} ${damaged === 1 ? 'file' : 'files'} damaged`
      : `${missing} ${missing === 1 ? 'file' : 'files'} missing`;

  return {
    installed,
    ready: false,
    statusLabel,
    actionLabel: installed ? 'Repair model' : 'Download model',
    filesLabel: `${GIGASTT_CATALOG_MODEL.fileCount} required`,
  };
}

export interface GigasttModelInstallAuthority {
  run: (install: () => Promise<void>) => Promise<void>;
}

export function createGigasttModelInstallAuthority(): GigasttModelInstallAuthority {
  let active: Promise<void> | null = null;

  return {
    run(install) {
      if (active) return active;

      let requested: Promise<void>;
      try {
        requested = install();
      } catch (error) {
        requested = Promise.reject(error);
      }
      const tracked = requested.finally(() => {
        if (active === tracked) active = null;
      });
      active = tracked;
      return tracked;
    },
  };
}

export type GigasttFinalizationOutcome = 'accepted' | 'disabled' | 'failed';

const ACTIVE_JOB_STATES = new Set<GigasttJobState>([
  'preparing_audio',
  'starting',
  'transcribing',
  'finalizing',
]);

export function requiresLiveTranscriptionModel(settings: GigasttSettings | null): boolean {
  return settings?.live_preview !== false;
}

export function isGigasttJobActive(snapshot: GigasttJobSnapshot | null): boolean {
  return snapshot !== null && ACTIVE_JOB_STATES.has(snapshot.state);
}

export function canAutomaticallyPostProcess(
  automaticPostProcessingRequested: boolean,
  snapshot: GigasttJobSnapshot | null | undefined,
  transcriptReady: boolean = false,
): boolean {
  if (!automaticPostProcessingRequested || snapshot === undefined) return false;
  return snapshot === null || (snapshot.state === 'ready' && transcriptReady);
}

export function canAutomaticallyGenerateSummary(
  postTranscriptionReady: boolean,
  shouldAutoLabelSpeakers: boolean,
  diarizationSettled: boolean,
): boolean {
  return postTranscriptionReady && (!shouldAutoLabelSpeakers || diarizationSettled);
}

export function canSummarizeCurrentDraft(snapshot: GigasttJobSnapshot | null): boolean {
  return snapshot?.state === 'failed' || snapshot?.state === 'cancelled';
}

/**
 * Merge a backend event without allowing another meeting or an older run to
 * overwrite the route currently on screen. A command response installs a new
 * run directly before subsequent events are merged through this function.
 */
export function mergeGigasttJobEvent(
  current: GigasttJobSnapshot | null,
  incoming: GigasttJobSnapshot,
  meetingId: string,
): GigasttJobSnapshot | null {
  if (incoming.meeting_id !== meetingId) return current;
  if (current && current.run_id !== incoming.run_id) return current;
  return incoming;
}

export function isGigasttModelReady(status: GigasttModelAvailability | null): boolean {
  return Boolean(
    status?.files.length
      && status.files.every(file => file.state === 'valid'),
  );
}

export function modelInstallPercent(progress: GigasttModelInstallProgress | null): number {
  if (!progress) return 0;
  if (progress.bytes_total && progress.bytes_total > 0) {
    const completedFiles = Math.max(0, progress.file_index - 1);
    const currentFile = progress.bytes_done / progress.bytes_total;
    const overall = progress.file_count > 0
      ? ((completedFiles + currentFile) / progress.file_count) * 100
      : currentFile * 100;
    return Math.min(100, Math.max(0, overall));
  }
  if (progress.file_count > 0) {
    const reportsCompletedFile = ['reused', 'verifying', 'complete'].includes(progress.phase);
    const completedFiles = reportsCompletedFile
      ? progress.file_index
      : Math.max(0, progress.file_index - 1);
    return Math.min(100, Math.max(0, (completedFiles / progress.file_count) * 100));
  }
  return 0;
}

export function recordingSaveDescription(
  outcome: GigasttFinalizationOutcome,
  draftSegments: number,
): string {
  if (outcome === 'accepted') {
    return 'Audio saved. Final GigaSTT transcription is continuing in the meeting view.';
  }
  if (outcome === 'disabled') {
    return draftSegments > 0
      ? `${draftSegments} draft transcript segments saved. Automatic final transcription is off.`
      : 'Audio saved. Automatic final transcription is off.';
  }
  return draftSegments > 0
    ? `Audio and ${draftSegments} draft transcript segments saved. Open the meeting to retry GigaSTT.`
    : 'Audio saved. Open the meeting to retry GigaSTT.';
}

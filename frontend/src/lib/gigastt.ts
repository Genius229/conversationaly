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
  cameFromRecording: boolean,
  snapshot: GigasttJobSnapshot | null | undefined,
  transcriptReady: boolean = false,
): boolean {
  if (!cameFromRecording || snapshot === undefined) return false;
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

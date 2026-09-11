'use client';

import { useState } from 'react';
import {
  AlertCircle,
  CheckCircle2,
  Download,
  LoaderCircle,
  RotateCcw,
  Square,
} from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Progress } from '@/components/ui/progress';
import { Switch } from '@/components/ui/switch';
import {
  canSummarizeCurrentDraft,
  isGigasttJobActive,
  isGigasttModelReady,
  modelInstallPercent,
} from '@/lib/gigastt';
import type { UseGigasttMeetingResult } from '@/hooks/useGigasttMeeting';

interface GigasttPanelProps {
  state: UseGigasttMeetingResult;
  hasCurrentTranscript: boolean;
  onSummarizeCurrentDraft: () => Promise<void>;
}

const JOB_LABELS = {
  preparing_audio: 'Preparing audio',
  starting: 'Starting GigaSTT',
  transcribing: 'Transcribing with GigaSTT',
  finalizing: 'Finalizing transcript',
  ready: 'Final transcript ready',
  failed: 'Final transcription failed',
  cancelled: 'Final transcription cancelled',
} as const;

function modelProblemCount(state: UseGigasttMeetingResult['modelStatus']): number {
  return state?.files.filter(file => file.state !== 'valid').length ?? 0;
}

export function GigasttPanel({
  state,
  hasCurrentTranscript,
  onSummarizeCurrentDraft,
}: GigasttPanelProps) {
  const [isSummarizingDraft, setIsSummarizingDraft] = useState(false);
  const jobActive = isGigasttJobActive(state.job ?? null);
  const modelReady = isGigasttModelReady(state.modelStatus);
  const modelDownloading = state.modelDownload.state === 'running';
  const installPercent = modelInstallPercent(state.modelDownload.progress);
  const canUseDraft = canSummarizeCurrentDraft(state.job ?? null) && hasCurrentTranscript;
  const finalTranscriptNeedsReload = state.job?.state === 'ready' && !state.transcriptReady;

  const handleSummarizeDraft = async () => {
    setIsSummarizingDraft(true);
    try {
      await onSummarizeCurrentDraft();
    } finally {
      setIsSummarizingDraft(false);
    }
  };

  const renderJobIcon = () => {
    if (state.job === undefined || jobActive || state.isRefreshingTranscript) {
      return <LoaderCircle className="size-4 shrink-0 animate-spin text-info-ink" aria-hidden="true" />;
    }
    if (finalTranscriptNeedsReload) {
      return <AlertCircle className="size-4 shrink-0 text-warn-ink" aria-hidden="true" />;
    }
    if (state.job?.state === 'ready') {
      return <CheckCircle2 className="size-4 shrink-0 text-brand" aria-hidden="true" />;
    }
    if (state.job?.state === 'failed' || state.job?.state === 'cancelled') {
      return <AlertCircle className="size-4 shrink-0 text-danger-ink" aria-hidden="true" />;
    }
    return <CheckCircle2 className="size-4 shrink-0 text-ink-faint" aria-hidden="true" />;
  };

  return (
    <section
      aria-label="GigaSTT final transcription"
      className="border-b border-line bg-panel px-5 py-3"
    >
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-x-6 gap-y-3">
        <div className="min-w-0 flex-1 basis-80">
          <div className="flex min-w-0 items-center gap-2" aria-live="polite">
            {renderJobIcon()}
            <h2 className="text-sm font-semibold text-ink">GigaSTT</h2>
            <span className="truncate text-sm text-ink-muted">
              {state.job === undefined
                ? 'Checking final transcription state'
                : state.job
                  ? state.job.state === 'ready' && !state.transcriptReady
                    ? state.isRefreshingTranscript
                      ? 'Final transcript ready — loading saved rows'
                      : 'Final transcript ready — reload needed'
                    : JOB_LABELS[state.job.state]
                  : 'No final transcription has run'}
              {state.job?.state === 'transcribing' && state.job.percent !== undefined
                ? ` — ${Math.round(state.job.percent)}%`
                : ''}
            </span>
          </div>

          {jobActive && (
            <Progress
              className="mt-2 h-1.5 max-w-xl bg-ink/10 [&>div]:bg-info"
              value={state.job?.percent ?? 0}
              aria-label="GigaSTT transcription progress"
              aria-valuetext={state.job?.percent === undefined ? JOB_LABELS[state.job!.state] : undefined}
            />
          )}

          {state.job?.message && (
            <p className="mt-1.5 max-w-3xl break-words text-xs text-danger-ink">
              {state.job.message}
            </p>
          )}
          {state.job?.cleanup_pending && (
            <p className="mt-1.5 text-xs text-warn-ink">
              Transcript saved; some file synchronization or cleanup is pending.
            </p>
          )}
          {state.error && (
            <p className="mt-1.5 max-w-3xl break-words text-xs text-danger-ink" role="alert">
              {state.error}
            </p>
          )}
        </div>

        <div className="flex flex-wrap items-center justify-end gap-2">
          {finalTranscriptNeedsReload && !state.isRefreshingTranscript && (
            <Button
              type="button"
              size="sm"
              variant="secondary"
              disabled={state.isActing}
              onClick={() => void state.retryTranscriptRefresh()}
            >
              <RotateCcw aria-hidden="true" />
              Retry loading transcript
            </Button>
          )}

          {jobActive ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={state.isActing}
              onClick={() => void state.cancelTranscription()}
            >
              <Square aria-hidden="true" />
              Cancel
            </Button>
          ) : (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={
                state.isActing
                || state.isRefreshingTranscript
                || !modelReady
                || modelDownloading
                || state.job === undefined
              }
              onClick={() => void state.startTranscription()}
            >
              <RotateCcw aria-hidden="true" />
              {state.job?.state === 'failed' || state.job?.state === 'cancelled'
                ? 'Retry final transcription'
                : state.job?.state === 'ready'
                  ? 'Re-transcribe with GigaSTT'
                  : 'Transcribe with GigaSTT'}
            </Button>
          )}

          {canUseDraft && (
            <Button
              type="button"
              size="sm"
              variant="secondary"
              disabled={isSummarizingDraft}
              onClick={() => void handleSummarizeDraft()}
            >
              {isSummarizingDraft && <LoaderCircle className="animate-spin" aria-hidden="true" />}
              Summarize current draft
            </Button>
          )}
        </div>
      </div>

      <div className="mt-3 flex flex-wrap items-center justify-between gap-x-6 gap-y-3 border-t border-line pt-3">
        <div className="min-w-0 flex-1 basis-80">
          <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2">
            <span className="text-xs font-medium text-ink">Model files</span>
            <span className="font-mono text-2xs text-ink-muted">
              {modelDownloading
                ? `${state.modelDownload.progress?.phase ?? 'Downloading'} — ${Math.round(installPercent)}%`
                : state.modelStatus === null
                  ? 'Checking local files'
                  : modelReady
                    ? 'Ready'
                    : `${modelProblemCount(state.modelStatus)} file(s) missing or damaged`}
            </span>
          </div>
          {modelDownloading && (
            <Progress
              className="mt-2 h-1.5 max-w-xl bg-ink/10"
              value={installPercent}
              aria-label="GigaSTT model download progress"
            />
          )}
          {state.modelDownload.message && state.modelDownload.state !== 'ready' && (
            <p className="mt-1.5 max-w-3xl break-words text-xs text-danger-ink">
              {state.modelDownload.message}
            </p>
          )}
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {modelDownloading ? (
            <Button
              type="button"
              size="sm"
              variant="ghost"
              disabled={state.isActing}
              onClick={() => void state.cancelModelInstall()}
            >
              Cancel download
            </Button>
          ) : !modelReady ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={state.isActing}
              onClick={() => void state.installModels()}
            >
              <Download aria-hidden="true" />
              {modelProblemCount(state.modelStatus) > 0 ? 'Download or repair models' : 'Download models'}
            </Button>
          ) : null}

          {state.settings && (
            <div className="flex flex-wrap items-center gap-x-4 gap-y-2 border-s border-line ps-4">
              <label className="flex items-center gap-2 text-xs text-ink-muted">
                <Switch
                  checked={state.settings.auto_transcribe}
                  disabled={state.isActing}
                  onCheckedChange={auto_transcribe => void state.saveSettings({
                    ...state.settings!,
                    auto_transcribe,
                  })}
                  aria-label="Automatically transcribe saved meetings with GigaSTT"
                />
                Auto final transcript
              </label>
              <label className="flex items-center gap-2 text-xs text-ink-muted">
                <Switch
                  checked={state.settings.live_preview}
                  disabled={state.isActing}
                  onCheckedChange={live_preview => void state.saveSettings({
                    ...state.settings!,
                    live_preview,
                  })}
                  aria-label="Show draft transcript while recording"
                />
                Live draft preview
              </label>
            </div>
          )}
        </div>
      </div>
    </section>
  );
}

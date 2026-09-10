'use client';

import { AlertCircle, AudioLines, CheckCircle2, LoaderCircle } from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Progress } from '@/components/ui/progress';
import type { UseGigasttModelResult } from '@/hooks/useGigasttModel';
import {
  GIGASTT_CATALOG_MODEL,
  gigasttCatalogView,
  modelInstallPercent,
} from '@/lib/gigastt';

interface GigasttModelCardProps {
  state: UseGigasttModelResult;
}

export function GigasttModelCard({ state }: GigasttModelCardProps) {
  const view = gigasttCatalogView(state.modelStatus, state.modelDownload);
  const downloading = state.modelDownload.state === 'running';
  const progress = modelInstallPercent(state.modelDownload.progress);
  const statusMessage = state.error
    ?? (state.modelDownload.state !== 'ready' ? state.modelDownload.message : null);

  return (
    <section aria-label="GigaSTT model" className="rounded-lg border border-line p-4">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0 flex-1 basis-80">
          <div className="flex flex-wrap items-center gap-2">
            <AudioLines className="size-4 text-info-ink" aria-hidden="true" />
            <h3 className="font-medium text-ink">
              {GIGASTT_CATALOG_MODEL.label}{' '}
              <span className="font-mono text-xs text-ink-muted">
                {GIGASTT_CATALOG_MODEL.version}
              </span>
            </h3>
            <span className="rounded-full bg-info-soft px-2 py-0.5 text-xs text-info-ink">
              {GIGASTT_CATALOG_MODEL.useTag}
            </span>
          </div>

          <p className="mt-1 text-sm text-ink-muted">
            {GIGASTT_CATALOG_MODEL.description}
          </p>
          <dl className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-ink-muted">
            <div className="flex gap-1">
              <dt>Language</dt>
              <dd className="font-medium text-ink">Russian</dd>
            </div>
            <div className="flex gap-1">
              <dt>Files</dt>
              <dd className="readout text-ink">{view.filesLabel}</dd>
            </div>
          </dl>
        </div>

        <div className="flex shrink-0 flex-wrap items-center justify-end gap-2">
          <span className="flex items-center gap-1.5 text-xs text-ink-muted" aria-live="polite">
            {downloading || state.modelStatus === null ? (
              <LoaderCircle className="size-4 animate-spin text-info-ink" aria-hidden="true" />
            ) : view.ready ? (
              <CheckCircle2 className="size-4 text-brand" aria-hidden="true" />
            ) : (
              <AlertCircle className="size-4 text-warn-ink" aria-hidden="true" />
            )}
            <span className="font-mono text-2xs">{view.statusLabel}</span>
          </span>

          {downloading ? (
            <Button
              type="button"
              size="sm"
              variant="ghost"
              disabled={state.isActing}
              onClick={() => void state.cancelModelInstall()}
            >
              {state.isActing ? 'Cancelling…' : 'Cancel download'}
            </Button>
          ) : view.actionLabel ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={state.isActing}
              onClick={() => void state.installModels()}
            >
              {state.isActing && <LoaderCircle className="animate-spin" aria-hidden="true" />}
              {state.isActing ? 'Starting…' : view.actionLabel}
            </Button>
          ) : null}
        </div>
      </div>

      {downloading && (
        <div className="mt-3">
          <Progress
            className="h-2 bg-ink/10"
            value={progress}
            aria-label="GigaSTT model download progress"
          />
          <p className="mt-1 text-xs text-ink-muted">
            {state.modelDownload.progress?.current_file
              ? `Downloading ${state.modelDownload.progress.current_file} · ${Math.round(progress)}%`
              : `Preparing download · ${Math.round(progress)}%`}
          </p>
        </div>
      )}

      {statusMessage && (
        <p className="mt-3 break-words rounded-md bg-danger-soft px-3 py-2 text-xs text-danger-ink" role="alert">
          {statusMessage}
        </p>
      )}
    </section>
  );
}

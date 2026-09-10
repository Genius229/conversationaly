import { useCallback, useEffect, useRef, useState } from 'react';
import { AlertCircle, Clock, FileAudio, HardDrive, Loader2, Upload } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { toast } from 'sonner';

import { GigasttModelCard } from '@/components/GigasttModelCard';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { getAudioFormatsDisplayList } from '@/constants/audioFormats';
import { useGigasttModel } from '@/hooks/useGigasttModel';
import { useImportAudio } from '@/hooks/useImportAudio';
import { canStartGigasttImport, importedMeetingRoute } from '@/lib/import-audio';
import { isGigasttModelReady, type GigasttJobSnapshot } from '@/lib/gigastt';

interface ImportAudioDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  preselectedFile?: string | null;
  onComplete?: () => void;
}

function formatDuration(seconds: number): string {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = Math.floor(seconds % 60);

  if (hours > 0) {
    return `${hours}:${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
  }
  return `${minutes}:${secs.toString().padStart(2, '0')}`;
}

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

export function ImportAudioDialog({
  open,
  onOpenChange,
  preselectedFile,
  onComplete,
}: ImportAudioDialogProps) {
  const router = useRouter();
  const { refetchMeetings } = useSidebar();
  const gigasttModel = useGigasttModel();
  const [title, setTitle] = useState('');
  const [titleModifiedByUser, setTitleModifiedByUser] = useState(false);
  const prevOpenRef = useRef(false);

  const handleImportAccepted = useCallback((snapshot: GigasttJobSnapshot) => {
    toast.success('Audio imported', {
      description: 'GigaSTT is preparing the final Russian transcript.',
    });
    void refetchMeetings();
    onComplete?.();
    onOpenChange(false);
    router.push(importedMeetingRoute(snapshot.meeting_id));
  }, [onComplete, onOpenChange, refetchMeetings, router]);

  const handleImportError = useCallback((error: string) => {
    toast.error('Import failed', { description: error });
  }, []);

  const {
    status,
    fileInfo,
    error,
    isProcessing,
    isBusy,
    selectFile,
    validateFile,
    startImport,
    reset,
  } = useImportAudio({
    onAccepted: handleImportAccepted,
    onError: handleImportError,
  });

  useEffect(() => {
    const wasOpen = prevOpenRef.current;
    prevOpenRef.current = open;

    if (!open && wasOpen) {
      reset();
      return;
    }

    if (open && !wasOpen) {
      reset();
      setTitle('');
      setTitleModifiedByUser(false);

      if (preselectedFile) {
        void validateFile(preselectedFile).then(info => {
          if (info) setTitle(info.filename);
        });
      }
    }
  }, [open, preselectedFile, reset, validateFile]);

  useEffect(() => {
    if (fileInfo && !title && !titleModifiedByUser) setTitle(fileInfo.filename);
  }, [fileInfo, title, titleModifiedByUser]);

  const handleSelectFile = async () => {
    const info = await selectFile();
    if (info) {
      setTitle(info.filename);
      setTitleModifiedByUser(false);
    }
  };

  const handleStartImport = async () => {
    if (!fileInfo) return;
    await startImport(fileInfo.path, title.trim() || fileInfo.filename);
  };

  const modelReady = isGigasttModelReady(gigasttModel.modelStatus);
  const canImport = canStartGigasttImport(Boolean(fileInfo), modelReady, isBusy);

  const handleOpenChange = (nextOpen: boolean) => {
    if (!nextOpen && isProcessing) return;
    if (!nextOpen) reset();
    onOpenChange(nextOpen);
  };

  const blockDismissalWhileArchiving = (event: Event) => {
    if (isProcessing) event.preventDefault();
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className="max-h-[calc(100vh-2rem)] overflow-y-auto sm:max-w-[560px]"
        onEscapeKeyDown={blockDismissalWhileArchiving}
        onInteractOutside={blockDismissalWhileArchiving}
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {isProcessing ? (
              <Loader2 className="size-5 animate-spin text-info-ink" aria-hidden="true" />
            ) : (
              <Upload className="size-5 text-info-ink" aria-hidden="true" />
            )}
            {isProcessing ? 'Preparing imported audio' : 'Import audio'}
          </DialogTitle>
          <DialogDescription>
            {isProcessing
              ? 'Copying the original audio into a new meeting before GigaSTT starts.'
              : 'Create a meeting from an existing recording. GigaSTT produces the final Russian transcript locally.'}
          </DialogDescription>
        </DialogHeader>

        {isProcessing ? (
          <div className="rounded-lg bg-sunken p-4" aria-live="polite">
            <p className="text-sm font-medium text-ink">Preserving the original audio…</p>
            <p className="mt-1 text-sm text-ink-muted">
              The meeting view will open as soon as its archived copy is ready. Transcription progress and Cancel remain available there.
            </p>
          </div>
        ) : (
          <div className="space-y-4 py-1">
            {fileInfo ? (
              <section aria-label="Selected audio file" className="space-y-3 rounded-lg bg-sunken p-4">
                <div className="flex min-w-0 items-start gap-3">
                  <FileAudio className="size-8 shrink-0 text-info-ink" aria-hidden="true" />
                  <div className="min-w-0 flex-1">
                    <p className="truncate font-medium text-ink" title={fileInfo.filename}>
                      {fileInfo.filename}
                    </p>
                    <dl className="mt-1 flex flex-wrap gap-x-4 gap-y-1 text-sm text-ink-muted">
                      <div className="flex items-center gap-1">
                        <Clock className="size-3.5" aria-hidden="true" />
                        <dt className="sr-only">Duration</dt>
                        <dd className="readout">{formatDuration(fileInfo.duration_seconds)}</dd>
                      </div>
                      <div className="flex items-center gap-1">
                        <HardDrive className="size-3.5" aria-hidden="true" />
                        <dt className="sr-only">File size</dt>
                        <dd className="readout">{formatFileSize(fileInfo.size_bytes)}</dd>
                      </div>
                      <div>
                        <dt className="sr-only">Format</dt>
                        <dd className="font-medium text-info-ink">{fileInfo.format}</dd>
                      </div>
                    </dl>
                  </div>
                </div>

                <div className="space-y-1.5">
                  <label htmlFor="import-meeting-title" className="text-sm font-medium text-ink">
                    Meeting title
                  </label>
                  <Input
                    id="import-meeting-title"
                    value={title}
                    onChange={event => {
                      setTitle(event.target.value);
                      setTitleModifiedByUser(true);
                    }}
                    placeholder="Enter meeting title"
                  />
                </div>

                <Button type="button" variant="outline" size="sm" onClick={() => void handleSelectFile()}>
                  Choose different file
                </Button>
              </section>
            ) : (
              <section
                aria-label="Audio file selection"
                className="rounded-lg border border-dashed border-line-strong p-7 text-center"
              >
                <FileAudio className="mx-auto mb-3 size-10 text-ink-faint" aria-hidden="true" />
                <Button
                  type="button"
                  onClick={() => void handleSelectFile()}
                  disabled={status === 'validating'}
                >
                  {status === 'validating' ? (
                    <>
                      <Loader2 className="animate-spin" aria-hidden="true" />
                      Validating…
                    </>
                  ) : (
                    <>
                      <Upload aria-hidden="true" />
                      Select audio file
                    </>
                  )}
                </Button>
                <p className="mt-2 text-sm text-ink-muted">{getAudioFormatsDisplayList()}</p>
              </section>
            )}

            <GigasttModelCard state={gigasttModel} />

            {error && (
              <div className="rounded-md bg-danger-soft px-3 py-2" role="alert">
                <div className="flex items-start gap-2">
                  <AlertCircle className="mt-0.5 size-4 shrink-0 text-danger-ink" aria-hidden="true" />
                  <div className="min-w-0">
                    <p className="text-sm font-medium text-danger-ink">The audio was not imported</p>
                    <p className="mt-0.5 break-words text-sm text-danger-ink">{error}</p>
                    {fileInfo && (
                      <p className="mt-1 text-xs text-danger-ink">
                        The selected file and title are still here. Fix the problem, then retry.
                      </p>
                    )}
                  </div>
                </div>
              </div>
            )}
          </div>
        )}

        <DialogFooter>
          {isProcessing ? (
            <Button type="button" disabled>
              <Loader2 className="animate-spin" aria-hidden="true" />
              Preparing meeting…
            </Button>
          ) : (
            <>
              <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button type="button" onClick={() => void handleStartImport()} disabled={!canImport}>
                <Upload aria-hidden="true" />
                {error ? 'Retry import' : 'Import with GigaSTT'}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

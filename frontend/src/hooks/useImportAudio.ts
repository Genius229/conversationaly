import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

import type { GigasttJobSnapshot } from '@/lib/gigastt';
import {
  createImportValidationAuthority,
  importErrorMessage,
  type ImportValidationAuthority,
} from '@/lib/import-audio';
import { applyPinnedSummaryLanguageToMeeting } from '@/lib/summary-language-preferences';
import { gigasttService } from '@/services/gigasttService';

export interface AudioFileInfo {
  path: string;
  filename: string;
  duration_seconds: number;
  size_bytes: number;
  format: string;
}

export type ImportStatus = 'idle' | 'validating' | 'processing' | 'error';

export interface UseImportAudioOptions {
  onAccepted?: (snapshot: GigasttJobSnapshot) => void;
  onError?: (error: string) => void;
}

export interface UseImportAudioReturn {
  status: ImportStatus;
  fileInfo: AudioFileInfo | null;
  error: string | null;
  isProcessing: boolean;
  isBusy: boolean;
  selectFile: () => Promise<AudioFileInfo | null>;
  validateFile: (path: string) => Promise<AudioFileInfo | null>;
  startImport: (sourcePath: string, title: string) => Promise<void>;
  reset: () => void;
}

export function useImportAudio({
  onAccepted,
  onError,
}: UseImportAudioOptions = {}): UseImportAudioReturn {
  const [status, setStatus] = useState<ImportStatus>('idle');
  const [fileInfo, setFileInfo] = useState<AudioFileInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const validationAuthorityRef = useRef<ImportValidationAuthority | null>(null);
  if (validationAuthorityRef.current === null) {
    validationAuthorityRef.current = createImportValidationAuthority();
  }
  const validationAuthority = validationAuthorityRef.current;

  const onAcceptedRef = useRef(onAccepted);
  const onErrorRef = useRef(onError);
  useEffect(() => { onAcceptedRef.current = onAccepted; }, [onAccepted]);
  useEffect(() => { onErrorRef.current = onError; }, [onError]);
  useEffect(() => () => validationAuthority.invalidate(), [validationAuthority]);

  const reportError = useCallback((value: unknown, fallback: string) => {
    const message = importErrorMessage(value, fallback);
    setStatus('error');
    setError(message);
    onErrorRef.current?.(message);
  }, []);

  const selectFile = useCallback(async (): Promise<AudioFileInfo | null> => {
    const operation = validationAuthority.begin();
    setStatus('validating');
    setError(null);

    try {
      const result = await invoke<AudioFileInfo | null>('select_and_validate_audio_command');
      if (!validationAuthority.isCurrent(operation)) return null;
      if (result) setFileInfo(result);
      setStatus('idle');
      return result;
    } catch (error) {
      if (!validationAuthority.isCurrent(operation)) return null;
      reportError(error, 'Failed to validate file');
      return null;
    }
  }, [reportError, validationAuthority]);

  const validateFile = useCallback(async (path: string): Promise<AudioFileInfo | null> => {
    const operation = validationAuthority.begin();
    setStatus('validating');
    setError(null);

    try {
      const result = await invoke<AudioFileInfo>('validate_audio_file_command', { path });
      if (!validationAuthority.isCurrent(operation)) return null;
      setFileInfo(result);
      setStatus('idle');
      return result;
    } catch (error) {
      if (!validationAuthority.isCurrent(operation)) return null;
      reportError(error, 'Failed to validate file');
      return null;
    }
  }, [reportError, validationAuthority]);

  const startImport = useCallback(async (sourcePath: string, title: string) => {
    setStatus('processing');
    setError(null);

    try {
      const snapshot = await gigasttService.importAudio(sourcePath, title);
      try {
        await applyPinnedSummaryLanguageToMeeting(snapshot.meeting_id);
      } catch (error) {
        console.warn('Failed to apply pinned summary language to imported meeting:', error);
        toast.warning('Could not apply default summary language', {
          description: 'The audio was imported, but the default summary language was not applied.',
        });
      }
      onAcceptedRef.current?.(snapshot);
    } catch (error) {
      reportError(error, 'Failed to import audio');
    }
  }, [reportError]);

  const reset = useCallback(() => {
    validationAuthority.invalidate();
    setStatus('idle');
    setFileInfo(null);
    setError(null);
  }, [validationAuthority]);

  return {
    status,
    fileInfo,
    error,
    isProcessing: status === 'processing',
    isBusy: status === 'processing' || status === 'validating',
    selectFile,
    validateFile,
    startImport,
    reset,
  };
}

const AUTOMATIC_POST_PROCESSING_SOURCES = new Set(['recording', 'import']);

export interface ImportValidationAuthority {
  begin: () => number;
  invalidate: () => void;
  isCurrent: (operation: number) => boolean;
}

export function createImportValidationAuthority(): ImportValidationAuthority {
  let current = 0;
  return {
    begin: () => ++current,
    invalidate: () => { current += 1; },
    isCurrent: operation => operation === current,
  };
}

export function isAutomaticPostProcessingSource(source: string | null): boolean {
  return source !== null && AUTOMATIC_POST_PROCESSING_SOURCES.has(source);
}

export function importedMeetingRoute(meetingId: string): string {
  return `/meeting-details?id=${encodeURIComponent(meetingId)}&source=import`;
}

export function gigasttImportCommandArgs(sourcePath: string, title: string) {
  return { sourcePath, title };
}

export function importErrorMessage(error: unknown, fallback: string): string {
  if (typeof error === 'string' && error) return error;
  if (
    typeof error === 'object'
    && error !== null
    && 'message' in error
    && typeof error.message === 'string'
    && error.message
  ) {
    return error.message;
  }
  const rendered = String(error);
  return rendered && rendered !== '[object Object]' ? rendered : fallback;
}

export function canStartGigasttImport(
  hasSelectedFile: boolean,
  modelReady: boolean,
  isBusy: boolean,
): boolean {
  return hasSelectedFile && modelReady && !isBusy;
}

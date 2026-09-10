import { useEffect, useState } from 'react';

import type { GigasttSettingsLoadState } from '@/lib/gigastt';
import { gigasttService } from '@/services/gigasttService';

/** Lightweight settings reader for the home capture surface. */
export function useGigasttSettings(): GigasttSettingsLoadState {
  const [state, setState] = useState<GigasttSettingsLoadState>({ status: 'loading' });

  useEffect(() => {
    let cancelled = false;
    let revision = 0;

    const unsubscribe = gigasttService.subscribeSettings(settings => {
      revision += 1;
      if (!cancelled) setState({ status: 'ready', settings });
    });

    const loadingRevision = revision;
    void gigasttService.getSettings().then(
      settings => {
        if (!cancelled && revision === loadingRevision) {
          setState({ status: 'ready', settings });
        }
      },
      () => {
        if (!cancelled && revision === loadingRevision) {
          setState({ status: 'error' });
        }
      },
    );

    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  return state;
}

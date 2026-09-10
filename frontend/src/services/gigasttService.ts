import { invoke } from '@tauri-apps/api/core';

import type {
  GigasttJobSnapshot,
  GigasttModelAvailability,
  GigasttModelDownloadState,
  GigasttSettings,
} from '@/lib/gigastt';

class GigasttService {
  getSettings(): Promise<GigasttSettings> {
    return invoke<GigasttSettings>('gigastt_get_settings');
  }

  saveSettings(settings: GigasttSettings): Promise<void> {
    return invoke<void>('gigastt_save_settings', { settings });
  }

  transcribeMeeting(meetingId: string): Promise<GigasttJobSnapshot> {
    return invoke<GigasttJobSnapshot>('gigastt_transcribe_meeting', { meetingId });
  }

  finalizeSavedMeeting(meetingId: string): Promise<GigasttJobSnapshot | null> {
    return invoke<GigasttJobSnapshot | null>('gigastt_finalize_saved_meeting', { meetingId });
  }

  cancelTranscription(meetingId: string): Promise<boolean> {
    return invoke<boolean>('gigastt_cancel_transcription', { meetingId });
  }

  getJobState(meetingId: string): Promise<GigasttJobSnapshot | null> {
    return invoke<GigasttJobSnapshot | null>('gigastt_get_job_state', { meetingId });
  }

  installModels(): Promise<void> {
    return invoke<void>('gigastt_install_models');
  }

  cancelModelInstall(): Promise<boolean> {
    return invoke<boolean>('gigastt_cancel_model_install');
  }

  getModelDownloadState(): Promise<GigasttModelDownloadState> {
    return invoke<GigasttModelDownloadState>('gigastt_model_download_state');
  }

  getModelStatus(): Promise<GigasttModelAvailability> {
    return invoke<GigasttModelAvailability>('gigastt_model_status');
  }
}

export const gigasttService = new GigasttService();

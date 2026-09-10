"use client";
import { useState, useEffect, useRef } from 'react';
import { Summary, SummaryResponse } from '@/types';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { TranscriptPanel } from '@/components/MeetingDetails/TranscriptPanel';
import { SummaryPanel } from '@/components/MeetingDetails/SummaryPanel';
import { ModelConfig } from '@/components/ModelSettingsModal';

// Custom hooks
import { useMeetingData } from '@/hooks/meeting-details/useMeetingData';
import { useSummaryGeneration } from '@/hooks/meeting-details/useSummaryGeneration';
import { useTemplates } from '@/hooks/meeting-details/useTemplates';
import { useCopyOperations } from '@/hooks/meeting-details/useCopyOperations';
import { useMeetingOperations } from '@/hooks/meeting-details/useMeetingOperations';
import { useConfig } from '@/contexts/ConfigContext';
import { useSpeakerLabelling } from '@/hooks/useSpeakerLabelling';
import { GigasttPanel } from '@/components/MeetingDetails/GigasttPanel';
import type { UseGigasttMeetingResult } from '@/hooks/useGigasttMeeting';
import { canAutomaticallyGenerateSummary } from '@/lib/gigastt';

export default function PageContent({
  meeting,
  summaryData,
  automaticPostProcessingRequested = false,
  automaticPostProcessingReady = false,
  gigastt,
  shouldAutoGenerate = false,
  onAutoGenerateComplete,
  onMeetingUpdated,
  onRefetchTranscripts,
  // Pagination props for efficient transcript loading
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
}: {
  meeting: any;
  summaryData: Summary | null;
  automaticPostProcessingRequested?: boolean;
  automaticPostProcessingReady?: boolean;
  gigastt: UseGigasttMeetingResult;
  shouldAutoGenerate?: boolean;
  onAutoGenerateComplete?: () => void;
  onMeetingUpdated?: () => Promise<void>;
  onRefetchTranscripts?: () => Promise<void>;
  // Pagination props
  segments?: any[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;
}) {
  console.log('📄 PAGE CONTENT: Initializing with data:', {
    meetingId: meeting.id,
    summaryDataKeys: summaryData ? Object.keys(summaryData) : null,
    transcriptsCount: meeting.transcripts?.length
  });

  // State
  const [customPrompt, setCustomPrompt] = useState<string>('');
  const [isRecording] = useState(false);
  const [summaryResponse] = useState<SummaryResponse | null>(null);

  // Ref to store the modal open function from SummaryGeneratorButtonGroup
  const openModelSettingsRef = useRef<(() => void) | null>(null);

  // Sidebar context
  const { serverAddress } = useSidebar();

  // Get model config from ConfigContext
  const { modelConfig, setModelConfig, isAutoLabelSpeakers } = useConfig();

  // Custom hooks
  const meetingData = useMeetingData({ meeting, summaryData, onMeetingUpdated });
  const templates = useTemplates();

  // Callback to register the modal open function
  const handleRegisterModalOpen = (openFn: () => void) => {
    console.log('📝 Registering modal open function in PageContent');
    openModelSettingsRef.current = openFn;
  };

  // Callback to trigger modal open (called from error handler)
  const handleOpenModelSettings = () => {
    console.log('🔔 Opening model settings from PageContent');
    if (openModelSettingsRef.current) {
      openModelSettingsRef.current();
    } else {
      console.warn('⚠️ Modal open function not yet registered');
    }
  };

  // Save model config to backend database and sync via event
  const handleSaveModelConfig = async (config?: ModelConfig) => {
    if (!config) return;
    try {
      await invoke('api_save_model_config', {
        provider: config.provider,
        model: config.model,
        whisperModel: config.whisperModel,
        apiKey: config.apiKey ?? null,
        ollamaEndpoint: config.ollamaEndpoint ?? null,
      });

      // Emit event so ConfigContext and other listeners stay in sync
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', config);

      toast.success('Model settings saved successfully');
    } catch (error) {
      console.error('Failed to save model config:', error);
      toast.error('Failed to save model settings');
    }
  };

  const summaryGeneration = useSummaryGeneration({
    meeting,
    transcripts: meetingData.transcripts,
    modelConfig: modelConfig,
    isModelConfigLoading: false, // ConfigContext loads on mount
    selectedTemplate: templates.selectedTemplate,
    onMeetingUpdated,
    updateMeetingTitle: meetingData.updateMeetingTitle,
    setAiSummary: meetingData.setAiSummary,
    onOpenModelSettings: handleOpenModelSettings,
  });

  const copyOperations = useCopyOperations({
    meeting,
    transcripts: meetingData.transcripts,
    meetingTitle: meetingData.meetingTitle,
    aiSummary: meetingData.aiSummary,
    blockNoteSummaryRef: meetingData.blockNoteSummaryRef,
  });

  const meetingOperations = useMeetingOperations({
    meeting,
  });

  const shouldAutoLabelSpeakers = isAutoLabelSpeakers && automaticPostProcessingRequested;
  const [automaticDiarizationSettled, setAutomaticDiarizationSettled] = useState(
    !shouldAutoLabelSpeakers,
  );
  const { labelSpeakers } = useSpeakerLabelling({
    meetingId: meeting.id,
    meetingFolderPath: meeting.folder_path,
    onComplete: async () => {
      try {
        await onRefetchTranscripts?.();
      } catch (error) {
        console.error('Speaker labels were saved but the transcript view could not refresh:', error);
        toast.error('Speaker labels saved, but the transcript view could not refresh');
      } finally {
        setAutomaticDiarizationSettled(true);
      }
    },
    onError: () => setAutomaticDiarizationSettled(true),
  });
  const automaticSummaryReady = canAutomaticallyGenerateSummary(
    automaticPostProcessingReady,
    shouldAutoLabelSpeakers,
    automaticDiarizationSettled,
  );

  useEffect(() => {
  }, []);

  // Auto-generate summary when flag is set
  useEffect(() => {
    let cancelled = false;

    const autoGenerate = async () => {
      if (
        shouldAutoGenerate
        && automaticPostProcessingReady
        && automaticSummaryReady
        && meetingData.transcripts.length > 0
        && !cancelled
      ) {
        console.log(`🤖 Auto-generating summary with ${modelConfig.provider}/${modelConfig.model}...`);
        await summaryGeneration.handleGenerateSummary('');

        // Notify parent that auto-generation is complete (only if not cancelled)
        if (onAutoGenerateComplete && !cancelled) {
          onAutoGenerateComplete();
        }
      }
    };

    autoGenerate();

    // Cleanup: cancel if component unmounts or meeting changes
    return () => {
      cancelled = true;
    };
  }, [
    shouldAutoGenerate,
    automaticPostProcessingReady,
    automaticSummaryReady,
    meeting.id,
  ]); // Re-run if final/optional speaker state changes for this meeting

  // When enabled, speaker labelling is part of the automatic sequence:
  // final GigaSTT rows -> diarization-complete -> refreshed labelled rows -> summary.
  const autoLabelledRef = useRef<string | null>(null);

  useEffect(() => {
    if (!automaticPostProcessingReady) {
      setAutomaticDiarizationSettled(false);
      return;
    }
    if (!shouldAutoLabelSpeakers || !meeting.folder_path || meetingData.transcripts.length === 0) {
      setAutomaticDiarizationSettled(true);
      return;
    }

    const authorityKey = `${meeting.id}:${gigastt.job?.run_id ?? 'current'}`;
    if (autoLabelledRef.current === authorityKey) return;
    autoLabelledRef.current = authorityKey;
    setAutomaticDiarizationSettled(false);

    let cancelled = false;
    const start = async () => {
      // downloadIfMissing stays false: enabling the setting does not silently
      // start a 139 MB download. A missing optional model settles as skipped.
      const accepted = await labelSpeakers({ downloadIfMissing: false });
      if (!accepted && !cancelled) setAutomaticDiarizationSettled(true);
    };
    void start();
    return () => {
      cancelled = true;
    };
  }, [
    automaticPostProcessingReady,
    shouldAutoLabelSpeakers,
    meeting.id,
    meeting.folder_path,
    meetingData.transcripts.length,
    gigastt.job?.run_id,
    labelSpeakers,
  ]);

  return (
    // No mount choreography — see /DESIGN.md → Motion.
    <div className="flex h-screen flex-col bg-canvas">
      <GigasttPanel
        state={gigastt}
        hasCurrentTranscript={meetingData.transcripts.length > 0}
        onSummarizeCurrentDraft={() => summaryGeneration.handleGenerateSummary('', {
          allowDraft: true,
        })}
      />
      <div className="flex flex-1 overflow-hidden">
        <TranscriptPanel
          transcripts={meetingData.transcripts}
          customPrompt={customPrompt}
          onPromptChange={setCustomPrompt}
          onCopyTranscript={copyOperations.handleCopyTranscript}
          onOpenMeetingFolder={meetingOperations.handleOpenMeetingFolder}
          isRecording={isRecording}
          disableAutoScroll={true}
          // Pagination props for efficient loading
          usePagination={true}
          segments={segments}
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          totalCount={totalCount}
          loadedCount={loadedCount}
          onLoadMore={onLoadMore}
          // Retranscription props
          meetingId={meeting.id}
          meetingFolderPath={meeting.folder_path}
          onRefetchTranscripts={onRefetchTranscripts}
        />
        <SummaryPanel
          meeting={meeting}
          meetingTitle={meetingData.meetingTitle}
          onTitleChange={meetingData.handleTitleChange}
          isEditingTitle={meetingData.isEditingTitle}
          onStartEditTitle={() => meetingData.setIsEditingTitle(true)}
          onFinishEditTitle={() => meetingData.setIsEditingTitle(false)}
          isTitleDirty={meetingData.isTitleDirty}
          summaryRef={meetingData.blockNoteSummaryRef}
          isSaving={meetingData.isSaving}
          onSaveAll={meetingData.saveAllChanges}
          onCopySummary={copyOperations.handleCopySummary}
          onOpenFolder={meetingOperations.handleOpenMeetingFolder}
          aiSummary={meetingData.aiSummary}
          summaryStatus={summaryGeneration.summaryStatus}
          transcripts={meetingData.transcripts}
          modelConfig={modelConfig}
          setModelConfig={setModelConfig}
          onSaveModelConfig={handleSaveModelConfig}
          onGenerateSummary={summaryGeneration.handleGenerateSummary}
          onStopGeneration={summaryGeneration.handleStopGeneration}
          customPrompt={customPrompt}
          summaryResponse={summaryResponse}
          onSaveSummary={meetingData.handleSaveSummary}
          onSummaryChange={meetingData.handleSummaryChange}
          onDirtyChange={meetingData.setIsSummaryDirty}
          summaryError={summaryGeneration.summaryError}
          onRegenerateSummary={summaryGeneration.handleRegenerateSummary}
          getSummaryStatusMessage={summaryGeneration.getSummaryStatusMessage}
          availableTemplates={templates.availableTemplates}
          selectedTemplate={templates.selectedTemplate}
          onTemplateSelect={templates.handleTemplateSelection}
          isModelConfigLoading={false}
          onOpenModelSettings={handleRegisterModalOpen}
        />
      </div>
    </div>
  );
}

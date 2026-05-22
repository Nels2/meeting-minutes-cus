import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useConfig } from '@/contexts/ConfigContext';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { recordingService } from '@/services/recordingService';
import Analytics from '@/lib/analytics';
import { showRecordingNotification } from '@/lib/recordingNotification';
import { toast } from 'sonner';

interface UseRecordingStartReturn {
  handleRecordingStart: (options?: RecordingStartOptions) => Promise<void>;
  isAutoStarting: boolean;
}

interface RecordingStartOptions {
  meetingName?: string;
  customContext?: string;
  source?: string;
}

interface O365CalendarEvent {
  title: string;
  start: string;
  end: string;
}

/**
 * Custom hook for managing recording start lifecycle.
 * Handles both manual start (button click) and auto-start (from sidebar navigation).
 *
 * Features:
 * - Meeting title generation (format: Meeting DD_MM_YY_HH_MM_SS)
 * - Transcript clearing on start
 * - Analytics tracking
 * - Recording notification display
 * - Auto-start from sidebar via sessionStorage flag
 */
export function useRecordingStart(
  isRecording: boolean,
  setIsRecording: (value: boolean) => void,
  showModal?: (name: 'modelSelector', message?: string) => void
): UseRecordingStartReturn {
  const [isAutoStarting, setIsAutoStarting] = useState(false);

  const { clearTranscripts, setMeetingTitle } = useTranscripts();
  const { setIsMeetingActive } = useSidebar();
  const { selectedDevices } = useConfig();
  const { setStatus } = useRecordingState();

  // Generate meeting title with timestamp
  const generateMeetingTitle = useCallback(() => {
    const now = new Date();
    const day = String(now.getDate()).padStart(2, '0');
    const month = String(now.getMonth() + 1).padStart(2, '0');
    const year = String(now.getFullYear()).slice(-2);
    const hours = String(now.getHours()).padStart(2, '0');
    const minutes = String(now.getMinutes()).padStart(2, '0');
    const seconds = String(now.getSeconds()).padStart(2, '0');
    return `Meeting ${day}_${month}_${year}_${hours}_${minutes}_${seconds}`;
  }, []);

  const resolveCalendarRecordingOptions = useCallback(async (options?: RecordingStartOptions): Promise<RecordingStartOptions> => {
    if (options?.meetingName || options?.customContext) {
      return options;
    }

    try {
      const event = await invoke<O365CalendarEvent | null>('calendar_get_current_o365_event');
      if (!event) return options || {};

      const context = await invoke<string>('calendar_build_event_context', { event });
      return {
        ...options,
        meetingName: event.title,
        customContext: context,
        source: options?.source || 'o365_calendar_auto_match',
      };
    } catch (error) {
      console.debug('No O365 calendar event matched this recording start:', error);
      return options || {};
    }
  }, []);

  const storePendingCalendarContext = useCallback((customContext?: string) => {
    if (typeof window !== 'undefined' && customContext?.trim()) {
      sessionStorage.setItem('pendingCalendarContext', customContext);
    }
  }, []);

  // Check if Parakeet transcription model is ready
  const checkParakeetReady = useCallback(async (): Promise<boolean> => {
    try {
      await invoke('parakeet_init');
      const hasModels = await invoke<boolean>('parakeet_has_available_models');
      return hasModels;
    } catch (error) {
      console.error('Failed to check Parakeet status:', error);
      return false;
    }
  }, []);

  // Check if any model is currently downloading
  const checkIfModelDownloading = useCallback(async (): Promise<boolean> => {
    try {
      const models = await invoke<any[]>('parakeet_get_available_models');
      const isDownloading = models.some(m =>
        m.status && (
          typeof m.status === 'object'
            ? 'Downloading' in m.status
            : m.status === 'Downloading'
        )
      );
      return isDownloading;
    } catch (error) {
      console.error('Failed to check model download status:', error);
      return false; // Default to not downloading (will show error + modal)
    }
  }, []);

  // Handle manual recording start (from button click)
  const handleRecordingStart = useCallback(async (options?: RecordingStartOptions) => {
    try {
      console.log('handleRecordingStart called - checking Parakeet model status');

      // Check if Parakeet transcription model is ready before starting
      const parakeetReady = await checkParakeetReady();
      if (!parakeetReady) {
        const isDownloading = await checkIfModelDownloading();
        if (isDownloading) {
          toast.info('Model download in progress', {
            description: 'Please wait for the transcription model to finish downloading before recording.',
            duration: 5000,
          });
          Analytics.trackButtonClick('start_recording_blocked_downloading', options?.source || 'home_page');
        } else {
          toast.error('Transcription model not ready', {
            description: 'Please download a transcription model before recording.',
            duration: 5000,
          });
          showModal?.('modelSelector', 'Transcription model setup required');
          Analytics.trackButtonClick('start_recording_blocked_missing', options?.source || 'home_page');
        }
        setStatus(RecordingStatus.IDLE);
        return;
      }

      console.log('Parakeet ready - setting up meeting title and state');

      const resolvedOptions = await resolveCalendarRecordingOptions(options);
      const randomTitle = resolvedOptions.meetingName || generateMeetingTitle();
      storePendingCalendarContext(resolvedOptions.customContext);
      setMeetingTitle(randomTitle);

      // Set STARTING status before initiating backend recording
      setStatus(RecordingStatus.STARTING, 'Initializing recording...');

      // Start the actual backend recording
      console.log('Starting backend recording with meeting:', randomTitle);
      await recordingService.startRecordingWithDevices(
        selectedDevices?.micDevice || null,
        selectedDevices?.systemDevice || null,
        randomTitle
      );
      console.log('Backend recording started successfully');

      // Update state after successful backend start
      // Note: RECORDING status will be set by RecordingStateContext event listener
      console.log('Setting isRecordingState to true');
      setIsRecording(true); // This will also update the sidebar via the useEffect
      clearTranscripts(); // Clear previous transcripts when starting new recording
      setIsMeetingActive(true);
      Analytics.trackButtonClick('start_recording', resolvedOptions.source || options?.source || 'home_page');

      // Show recording notification if enabled
      await showRecordingNotification();
    } catch (error) {
      console.error('Failed to start recording:', error);
      setStatus(RecordingStatus.ERROR, error instanceof Error ? error.message : 'Failed to start recording');
      setIsRecording(false); // Reset state on error
      Analytics.trackButtonClick('start_recording_error', options?.source || 'home_page');
      // Re-throw so RecordingControls can handle device-specific errors
      throw error;
    }
  }, [generateMeetingTitle, setMeetingTitle, setIsRecording, clearTranscripts, setIsMeetingActive, checkParakeetReady, checkIfModelDownloading, selectedDevices, showModal, setStatus, resolveCalendarRecordingOptions, storePendingCalendarContext]);

  // Check for autoStartRecording flag and start recording automatically
  useEffect(() => {
    const checkAutoStartRecording = async () => {
      if (typeof window !== 'undefined') {
        const shouldAutoStart = sessionStorage.getItem('autoStartRecording');
        if (shouldAutoStart === 'true' && !isRecording && !isAutoStarting) {
          console.log('Auto-starting recording from navigation...');
          setIsAutoStarting(true);
          sessionStorage.removeItem('autoStartRecording'); // Clear the flag
          const pendingOptionsRaw = sessionStorage.getItem('pendingRecordingOptions');
          let pendingOptions: RecordingStartOptions | undefined;
          if (pendingOptionsRaw) {
            try {
              pendingOptions = JSON.parse(pendingOptionsRaw) as RecordingStartOptions;
            } catch (error) {
              console.warn('Ignoring invalid pending recording options:', error);
            }
          }
          sessionStorage.removeItem('pendingRecordingOptions');

          // Check if Parakeet transcription model is ready before starting
          const parakeetReady = await checkParakeetReady();
          if (!parakeetReady) {
            const isDownloading = await checkIfModelDownloading();
            if (isDownloading) {
              toast.info('Model download in progress', {
                description: 'Please wait for the transcription model to finish downloading before recording.',
                duration: 5000,
              });
              Analytics.trackButtonClick('start_recording_blocked_downloading', 'sidebar_auto');
            } else {
              toast.error('Transcription model not ready', {
                description: 'Please download a transcription model before recording.',
                duration: 5000,
              });
              showModal?.('modelSelector', 'Transcription model setup required');
              Analytics.trackButtonClick('start_recording_blocked_missing', 'sidebar_auto');
            }
            setStatus(RecordingStatus.IDLE);
            setIsAutoStarting(false);
            return;
          }

          // Start the actual backend recording
          try {
            // Generate meeting title
            const resolvedOptions = await resolveCalendarRecordingOptions({
              ...pendingOptions,
              source: pendingOptions?.source || 'sidebar_auto',
            });
            const generatedMeetingTitle = resolvedOptions.meetingName || generateMeetingTitle();
            storePendingCalendarContext(resolvedOptions.customContext);

            // Set STARTING status before initiating backend recording
            setStatus(RecordingStatus.STARTING, 'Initializing recording...');

            console.log('Auto-starting backend recording with meeting:', generatedMeetingTitle);
            const result = await recordingService.startRecordingWithDevices(
              selectedDevices?.micDevice || null,
              selectedDevices?.systemDevice || null,
              generatedMeetingTitle
            );
            console.log('Auto-start backend recording result:', result);

            // Update UI state after successful backend start
            // Note: RECORDING status will be set by RecordingStateContext event listener
            setMeetingTitle(generatedMeetingTitle);
            setIsRecording(true);
            clearTranscripts();
            setIsMeetingActive(true);
            Analytics.trackButtonClick('start_recording', resolvedOptions.source || 'sidebar_auto');

            // Show recording notification if enabled
            await showRecordingNotification();
          } catch (error) {
            console.error('Failed to auto-start recording:', error);
            setStatus(RecordingStatus.ERROR, error instanceof Error ? error.message : 'Failed to auto-start recording');
            alert('Failed to start recording. Check console for details.');
            Analytics.trackButtonClick('start_recording_error', 'sidebar_auto');
          } finally {
            setIsAutoStarting(false);
          }
        }
      }
    };

    checkAutoStartRecording();
  }, [
    isRecording,
    isAutoStarting,
    selectedDevices,
    generateMeetingTitle,
    setMeetingTitle,
    setIsRecording,
    clearTranscripts,
    setIsMeetingActive,
    checkParakeetReady,
    checkIfModelDownloading,
    showModal,
    setStatus,
    resolveCalendarRecordingOptions,
    storePendingCalendarContext,
  ]);

  // Listen for direct recording trigger from sidebar when already on home page
  useEffect(() => {
    const handleDirectStart = async (event: Event) => {
      if (isRecording || isAutoStarting) {
        console.log('Recording already in progress, ignoring direct start event');
        return;
      }

      const detail = (event as CustomEvent<RecordingStartOptions>).detail;
      const source = detail?.source || 'sidebar_direct';
      console.log('Direct start from sidebar - checking Parakeet model status');
      setIsAutoStarting(true);

      // Check if Parakeet transcription model is ready before starting
      const parakeetReady = await checkParakeetReady();
      if (!parakeetReady) {
        const isDownloading = await checkIfModelDownloading();
        if (isDownloading) {
          toast.info('Model download in progress', {
            description: 'Please wait for the transcription model to finish downloading before recording.',
            duration: 5000,
          });
          Analytics.trackButtonClick('start_recording_blocked_downloading', source);
        } else {
          toast.error('Transcription model not ready', {
            description: 'Please download a transcription model before recording.',
            duration: 5000,
          });
          showModal?.('modelSelector', 'Transcription model setup required');
          Analytics.trackButtonClick('start_recording_blocked_missing', source);
        }
        setStatus(RecordingStatus.IDLE);
        setIsAutoStarting(false);
        return;
      }

      try {
        // Generate meeting title
        const resolvedOptions = await resolveCalendarRecordingOptions(detail);
        const generatedMeetingTitle = resolvedOptions.meetingName || generateMeetingTitle();
        storePendingCalendarContext(resolvedOptions.customContext);

        // Set STARTING status before initiating backend recording
        setStatus(RecordingStatus.STARTING, 'Initializing recording...');

        console.log('Starting backend recording with meeting:', generatedMeetingTitle);
        const result = await recordingService.startRecordingWithDevices(
          selectedDevices?.micDevice || null,
          selectedDevices?.systemDevice || null,
          generatedMeetingTitle
        );
        console.log('Backend recording result:', result);

        // Update UI state after successful backend start
        // Note: RECORDING status will be set by RecordingStateContext event listener
        setMeetingTitle(generatedMeetingTitle);
        setIsRecording(true);
        clearTranscripts();
        setIsMeetingActive(true);
        Analytics.trackButtonClick('start_recording', resolvedOptions.source || source);

        // Show recording notification if enabled
        await showRecordingNotification();
      } catch (error) {
        console.error('Failed to start recording from sidebar:', error);
        setStatus(RecordingStatus.ERROR, error instanceof Error ? error.message : 'Failed to start recording from sidebar');
        alert('Failed to start recording. Check console for details.');
        Analytics.trackButtonClick('start_recording_error', source);
      } finally {
        setIsAutoStarting(false);
      }
    };

    window.addEventListener('start-recording-from-sidebar', handleDirectStart);

    return () => {
      window.removeEventListener('start-recording-from-sidebar', handleDirectStart);
    };
  }, [
    isRecording,
    isAutoStarting,
    selectedDevices,
    generateMeetingTitle,
    setMeetingTitle,
    setIsRecording,
    clearTranscripts,
    setIsMeetingActive,
    checkParakeetReady,
    checkIfModelDownloading,
    showModal,
    setStatus,
    resolveCalendarRecordingOptions,
    storePendingCalendarContext,
  ]);

  return {
    handleRecordingStart,
    isAutoStarting,
  };
}

'use client';

import { useCallback, useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Bell, Monitor, Play, RotateCw, Square, Users, Video } from 'lucide-react';
import { Label } from '@/components/ui/label';
import { Switch } from '@/components/ui/switch';
import { Button } from '@/components/ui/button';
import { toast } from 'sonner';

interface MeetingDetectionSettings {
  enabled: boolean;
  auto_start_recording: boolean;
  auto_stop_recording: boolean;
  detect_zoom: boolean;
  detect_teams: boolean;
  detect_google_meet: boolean;
  notify_on_detection: boolean;
  poll_interval_secs: number;
}

interface DetectedMeeting {
  app_name: string;
  process_name: string;
  detected_at: string;
  is_active_meeting: boolean;
}

interface MeetingDetectionStatus {
  is_monitoring: boolean;
  current_meeting: DetectedMeeting | null;
  settings: MeetingDetectionSettings;
  auto_recording_active: boolean;
}

type ToggleKey = Exclude<keyof MeetingDetectionSettings, 'poll_interval_secs'>;

const defaultSettings: MeetingDetectionSettings = {
  enabled: false,
  auto_start_recording: false,
  auto_stop_recording: true,
  detect_zoom: true,
  detect_teams: true,
  detect_google_meet: true,
  notify_on_detection: true,
  poll_interval_secs: 5,
};

export function MeetingDetectionSettings() {
  const [settings, setSettings] = useState<MeetingDetectionSettings>(defaultSettings);
  const [status, setStatus] = useState<MeetingDetectionStatus | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);

  const refreshStatus = useCallback(async () => {
    const [loadedSettings, loadedStatus] = await Promise.all([
      invoke<MeetingDetectionSettings>('get_meeting_detection_settings'),
      invoke<MeetingDetectionStatus>('get_meeting_detection_status'),
    ]);

    setSettings(loadedSettings);
    setStatus(loadedStatus);
  }, []);

  useEffect(() => {
    refreshStatus()
      .catch((error) => {
        console.error('Failed to load meeting detection settings:', error);
        toast.error('Failed to load meeting detection settings');
      })
      .finally(() => setIsLoading(false));
  }, [refreshStatus]);

  useEffect(() => {
    const unsubscribers: Array<() => void> = [];

    const setupListeners = async () => {
      const unlistenDetected = await listen<DetectedMeeting>('meeting-detected', (event) => {
        setStatus((prev) =>
          prev ? { ...prev, current_meeting: event.payload } : prev
        );
      });
      unsubscribers.push(unlistenDetected);

      const unlistenEnded = await listen('meeting-ended', () => {
        setStatus((prev) =>
          prev ? { ...prev, current_meeting: null, auto_recording_active: false } : prev
        );
      });
      unsubscribers.push(unlistenEnded);
    };

    setupListeners().catch((error) => {
      console.error('Failed to setup meeting detection settings listeners:', error);
    });

    return () => unsubscribers.forEach((unsubscribe) => unsubscribe());
  }, []);

  const updateSettings = useCallback(async (nextSettings: MeetingDetectionSettings) => {
    setIsSaving(true);
    try {
      await invoke('set_meeting_detection_settings', { settings: nextSettings });
      setSettings(nextSettings);
      const nextStatus = await invoke<MeetingDetectionStatus>('get_meeting_detection_status');
      setStatus(nextStatus);
    } catch (error) {
      console.error('Failed to save meeting detection settings:', error);
      toast.error('Failed to save meeting detection settings');
    } finally {
      setIsSaving(false);
    }
  }, []);

  const handleToggle = (key: ToggleKey) => {
    updateSettings({ ...settings, [key]: !settings[key] });
  };

  const checkNow = async () => {
    try {
      const meeting = await invoke<DetectedMeeting | null>('check_for_active_meeting');
      if (meeting) {
        setStatus((prev) => prev ? { ...prev, current_meeting: meeting } : prev);
        toast.success(`${meeting.app_name} meeting detected`);
      } else {
        toast.info('No active meeting detected');
      }
    } catch (error) {
      console.error('Failed to check for active meeting:', error);
      toast.error('Meeting check failed');
    }
  };

  if (isLoading) {
    return <div className="p-6 text-sm text-gray-600 dark:text-gray-400">Loading auto-detection settings...</div>;
  }

  return (
    <div className="space-y-6">
      <div className="rounded-lg border border-gray-200 bg-white p-6 shadow-sm dark:border-gray-800 dark:bg-gray-900">
        <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
          <div>
            <h3 className="text-lg font-semibold text-gray-900 dark:text-gray-100">Meeting Auto-Detection</h3>
            <p className="mt-1 text-sm text-gray-600 dark:text-gray-400">
              Detect active Zoom, Microsoft Teams, and Google Meet meetings on this computer.
            </p>
          </div>
          <div className="flex items-center gap-3">
            <Button type="button" variant="outline" size="sm" onClick={checkNow} disabled={isSaving}>
              <RotateCw className="mr-2 h-4 w-4" />
              Check Now
            </Button>
            <Switch
              id="meeting-detection-enabled"
              checked={settings.enabled}
              onCheckedChange={() => handleToggle('enabled')}
              disabled={isSaving}
            />
            <Label htmlFor="meeting-detection-enabled" className="font-medium">
              {settings.enabled ? 'Enabled' : 'Disabled'}
            </Label>
          </div>
        </div>

        {settings.enabled && status && (
          <div className={`mt-5 rounded-lg border p-4 ${
            status.current_meeting
              ? 'border-green-200 bg-green-50 dark:border-green-900 dark:bg-green-950/40'
              : 'border-gray-200 bg-gray-50 dark:border-gray-800 dark:bg-gray-800'
          }`}>
            <div className="flex items-center gap-3">
              <div className={`h-3 w-3 rounded-full ${status.current_meeting ? 'animate-pulse bg-green-500' : 'bg-gray-400'}`} />
              <div>
                <p className={`font-medium ${status.current_meeting ? 'text-green-800 dark:text-green-200' : 'text-gray-700 dark:text-gray-200'}`}>
                  {status.current_meeting
                    ? `${status.current_meeting.app_name} meeting detected`
                    : 'Monitoring for meetings'}
                </p>
                <p className={`text-sm ${status.current_meeting ? 'text-green-700 dark:text-green-300' : 'text-gray-500 dark:text-gray-400'}`}>
                  {status.current_meeting
                    ? status.auto_recording_active ? 'Auto-recording is active' : status.current_meeting.process_name
                    : `Checking every ${settings.poll_interval_secs} seconds`}
                </p>
              </div>
            </div>
          </div>
        )}
      </div>

      <SettingsGroup title="Applications to Detect">
        <ToggleRow
          icon={<Video className="h-5 w-5 text-blue-500" />}
          label="Zoom"
          description="Detect Zoom meetings on Windows using meeting processes and window titles."
          checked={settings.detect_zoom}
          disabled={isSaving || !settings.enabled}
          onToggle={() => handleToggle('detect_zoom')}
        />
        <ToggleRow
          icon={<Users className="h-5 w-5 text-violet-500" />}
          label="Microsoft Teams"
          description="Detect Teams desktop calls and meetings."
          checked={settings.detect_teams}
          disabled={isSaving || !settings.enabled}
          onToggle={() => handleToggle('detect_teams')}
        />
        <ToggleRow
          icon={<Monitor className="h-5 w-5 text-green-500" />}
          label="Google Meet"
          description="Detect Google Meet from visible browser window titles."
          checked={settings.detect_google_meet}
          disabled={isSaving || !settings.enabled}
          onToggle={() => handleToggle('detect_google_meet')}
        />
      </SettingsGroup>

      <SettingsGroup title="Recording Behavior">
        <ToggleRow
          icon={<Play className="h-5 w-5 text-red-500" />}
          label="Auto-start Recording"
          description="Start recording automatically when a meeting is detected."
          checked={settings.auto_start_recording}
          disabled={isSaving || !settings.enabled}
          onToggle={() => handleToggle('auto_start_recording')}
        />
        <ToggleRow
          icon={<Square className="h-5 w-5 text-gray-500" />}
          label="Auto-stop Recording"
          description="Stop recording automatically when the detected meeting ends."
          checked={settings.auto_stop_recording}
          disabled={isSaving || !settings.enabled}
          onToggle={() => handleToggle('auto_stop_recording')}
        />
        <ToggleRow
          icon={<Bell className="h-5 w-5 text-yellow-500" />}
          label="Detection Notifications"
          description="Show a notification when a meeting is detected."
          checked={settings.notify_on_detection}
          disabled={isSaving || !settings.enabled}
          onToggle={() => handleToggle('notify_on_detection')}
        />
      </SettingsGroup>

      <div className="rounded-lg border border-blue-200 bg-blue-50 p-4 dark:border-blue-900 dark:bg-blue-950/40">
        <p className="text-sm text-blue-800 dark:text-blue-200">
          <strong>Privacy Note:</strong> Detection checks local process names and visible window titles.
          It does not access meeting content, audio, or video before recording starts.
        </p>
      </div>
    </div>
  );
}

function SettingsGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="rounded-lg border border-gray-200 bg-white p-6 shadow-sm dark:border-gray-800 dark:bg-gray-900">
      <h4 className="mb-4 font-medium text-gray-900 dark:text-gray-100">{title}</h4>
      <div className="space-y-4">{children}</div>
    </div>
  );
}

function ToggleRow({
  icon,
  label,
  description,
  checked,
  disabled,
  onToggle,
}: {
  icon: ReactNode;
  label: string;
  description: string;
  checked: boolean;
  disabled: boolean;
  onToggle: () => void;
}) {
  const id = `meeting-detection-${label.toLowerCase().replace(/\s+/g, '-')}`;

  return (
    <div className="flex items-center justify-between gap-4">
      <div className="flex items-center gap-3">
        {icon}
        <div>
          <Label htmlFor={id} className="font-medium text-gray-900 dark:text-gray-100">
            {label}
          </Label>
          <p className="text-sm text-gray-500 dark:text-gray-400">{description}</p>
        </div>
      </div>
      <Switch id={id} checked={checked} disabled={disabled} onCheckedChange={onToggle} />
    </div>
  );
}

export default MeetingDetectionSettings;

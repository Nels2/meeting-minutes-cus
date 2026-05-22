'use client';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useRouter } from 'next/navigation';
import { CalendarDays, Copy, ExternalLink, Link2, Loader2, LogOut, Play, RefreshCw, Save } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Textarea } from '@/components/ui/textarea';
import { toast } from 'sonner';

interface O365CalendarSettingsValue {
  tenantId: string;
  clientId: string;
  redirectUri: string;
  scopes: string;
}

interface O365CalendarEvent {
  id: string;
  title: string;
  joinUrl?: string | null;
  participants: string[];
  description?: string | null;
  start: string;
  end: string;
  webLink?: string | null;
}

interface O365CalendarState {
  settings: O365CalendarSettingsValue;
  connected: boolean;
  lastEvents: O365CalendarEvent[];
}

interface O365CalendarSignInResult {
  connected: boolean;
  authUrl: string;
  manualFallback: boolean;
}

const defaultSettings: O365CalendarSettingsValue = {
  tenantId: '',
  clientId: '',
  redirectUri: 'http://localhost',
  scopes: 'openid profile offline_access User.Read Calendars.Read',
};

const DEFAULT_SCOPES = defaultSettings.scopes;
const RECOMMENDED_REDIRECT_URI = defaultSettings.redirectUri;

function normalizeSettings(settings: O365CalendarSettingsValue): O365CalendarSettingsValue {
  return {
    tenantId: settings.tenantId.trim(),
    clientId: settings.clientId.trim(),
    redirectUri: settings.redirectUri.trim(),
    scopes: settings.scopes.trim().replace(/\s+/g, ' ') || DEFAULT_SCOPES,
  };
}

function validateRedirectUri(redirectUri: string): string | null {
  const value = redirectUri.trim();
  const lower = value.toLowerCase();

  if (!value) {
    return 'Redirect URI is required. Use http://localhost for this desktop app.';
  }

  if (
    lower.includes('login.microsoftonline.com') ||
    lower.includes('/oauth2/') ||
    lower.includes('/authorize') ||
    lower.includes('/token')
  ) {
    return 'Redirect URI must be http://localhost, not a Microsoft authorize or token endpoint.';
  }

  try {
    const url = new URL(value);
    if (url.protocol !== 'http:' || (url.hostname !== 'localhost' && url.hostname !== '127.0.0.1')) {
      return 'Redirect URI must be http://localhost or http://127.0.0.1 for this desktop app.';
    }
  } catch {
    return 'Redirect URI must be a valid URL, for example http://localhost.';
  }

  return null;
}

function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string' && error.trim()) return error;
  return fallback;
}

function formatEventTime(value: string): string {
  const date = new Date(value.endsWith('Z') ? value : `${value}Z`);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString([], {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  });
}

export function O365CalendarSettings() {
  const router = useRouter();
  const [settings, setSettings] = useState<O365CalendarSettingsValue>(defaultSettings);
  const [events, setEvents] = useState<O365CalendarEvent[]>([]);
  const [connected, setConnected] = useState(false);
  const [redirectUrl, setRedirectUrl] = useState('');
  const [lastAuthUrl, setLastAuthUrl] = useState('');
  const [isBusy, setIsBusy] = useState(false);
  const [connectStatus, setConnectStatus] = useState('');

  const loadState = useCallback(async () => {
    const state = await invoke<O365CalendarState>('calendar_get_o365_settings');
    setSettings(normalizeSettings({ ...defaultSettings, ...state.settings }));
    setConnected(state.connected);
    setEvents(state.lastEvents || []);
  }, []);

  useEffect(() => {
    loadState().catch((error) => {
      console.error('Failed to load O365 calendar settings:', error);
      toast.error('Failed to load calendar settings');
    });
  }, [loadState]);

  const saveSettings = async () => {
    setIsBusy(true);
    try {
      const normalizedSettings = normalizeSettings(settings);
      const redirectError = validateRedirectUri(normalizedSettings.redirectUri);
      if (redirectError) throw new Error(redirectError);
      setSettings(normalizedSettings);
      await invoke('calendar_save_o365_settings', { settings: normalizedSettings });
      toast.success('Calendar settings saved');
    } catch (error) {
      console.error('Failed to save calendar settings:', error);
      toast.error(errorMessage(error, 'Failed to save calendar settings'));
    } finally {
      setIsBusy(false);
    }
  };

  const connect = async () => {
    setIsBusy(true);
    setConnectStatus('Waiting for Microsoft sign-in...');
    try {
      const normalizedSettings = normalizeSettings(settings);
      const redirectError = validateRedirectUri(normalizedSettings.redirectUri);
      if (redirectError) throw new Error(redirectError);
      setSettings(normalizedSettings);
      const result = await invoke<O365CalendarSignInResult>('calendar_start_o365_sign_in', {
        settings: normalizedSettings,
      });
      if (!result.authUrl.includes('/oauth2/v2.0/authorize?') || !result.authUrl.includes('scope=')) {
        throw new Error('Generated Microsoft sign-in URL is missing the OAuth scope parameter');
      }
      setLastAuthUrl(result.authUrl);

      if (result.manualFallback || !result.connected) {
        toast.info('Sign in opened in your browser. Paste the full redirect URL here when it finishes.');
        return;
      }

      setConnected(true);
      toast.success('Microsoft 365 calendar connected');
      await refreshEvents();
    } catch (error) {
      console.error('Failed to start O365 sign-in:', error);
      toast.error(errorMessage(error, 'Failed to start O365 sign-in'));
    } finally {
      setConnectStatus('');
      setIsBusy(false);
    }
  };

  const completeSignIn = async () => {
    if (!redirectUrl.trim()) {
      toast.error('Paste the full redirect URL from Microsoft first');
      return;
    }

    setIsBusy(true);
    try {
      await invoke('calendar_exchange_o365_redirect', { redirectUrl });
      setRedirectUrl('');
      setConnected(true);
      toast.success('Microsoft 365 calendar connected');
      await refreshEvents();
    } catch (error) {
      console.error('Failed to complete O365 sign-in:', error);
      toast.error(errorMessage(error, 'Failed to complete O365 sign-in'));
    } finally {
      setIsBusy(false);
    }
  };

  const disconnect = async () => {
    setIsBusy(true);
    try {
      await invoke('calendar_disconnect_o365');
      setConnected(false);
      setEvents([]);
      toast.success('Calendar disconnected');
    } catch (error) {
      console.error('Failed to disconnect calendar:', error);
      toast.error('Failed to disconnect calendar');
    } finally {
      setIsBusy(false);
    }
  };

  const testConnection = async () => {
    setIsBusy(true);
    try {
      await invoke('calendar_test_o365_connection');
      toast.success('Microsoft Graph connection succeeded');
    } catch (error) {
      console.error('Calendar connection test failed:', error);
      toast.error(errorMessage(error, 'Connection test failed'));
    } finally {
      setIsBusy(false);
    }
  };

  const refreshEvents = async () => {
    setIsBusy(true);
    try {
      const nextEvents = await invoke<O365CalendarEvent[]>('calendar_fetch_o365_events', {
        daysBefore: 1,
        daysAfter: 14,
      });
      setEvents(nextEvents);
      toast.success(`Loaded ${nextEvents.length} calendar events`);
    } catch (error) {
      console.error('Failed to fetch calendar events:', error);
      toast.error(errorMessage(error, 'Failed to fetch calendar events'));
    } finally {
      setIsBusy(false);
    }
  };

  const openEvent = async (event: O365CalendarEvent) => {
    const url = event.joinUrl || event.webLink;
    if (!url) {
      toast.error('This event does not have a meeting link');
      return;
    }
    await invoke('open_external_url', { url });
  };

  const startRecordingFromEvent = async (event: O365CalendarEvent) => {
    try {
      const context = await invoke<string>('calendar_build_event_context', { event });
      sessionStorage.setItem('pendingCalendarContext', context);
      sessionStorage.setItem('pendingRecordingOptions', JSON.stringify({
        meetingName: event.title,
        customContext: context,
        source: 'o365_calendar',
      }));
      sessionStorage.setItem('autoStartRecording', 'true');

      if (event.joinUrl) {
        await invoke('open_external_url', { url: event.joinUrl });
      }

      window.dispatchEvent(new CustomEvent('start-recording-from-sidebar', {
        detail: {
          meetingName: event.title,
          customContext: context,
          source: 'o365_calendar',
        },
      }));
      router.push('/');
    } catch (error) {
      console.error('Failed to start recording from calendar event:', error);
      toast.error('Failed to start recording from calendar event');
    }
  };

  return (
    <div className="space-y-6">
      <div className="rounded-lg border border-gray-200 bg-white p-6 shadow-sm dark:border-gray-800 dark:bg-gray-900">
        <div className="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
          <div>
            <h3 className="text-lg font-semibold text-gray-900 dark:text-gray-100">Microsoft 365 Calendar</h3>
            <p className="mt-1 text-sm text-gray-600 dark:text-gray-400">
              Connect a custom Entra ID app with delegated Graph calendar access.
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button type="button" variant="outline" size="sm" onClick={saveSettings} disabled={isBusy}>
              <Save className="mr-2 h-4 w-4" />
              Save
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={connect}
              disabled={isBusy}
              className="bg-blue-600 text-white hover:bg-blue-700 dark:bg-blue-500 dark:text-white dark:hover:bg-blue-600"
            >
              {connectStatus ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : <Link2 className="mr-2 h-4 w-4" />}
              {connectStatus || 'Connect'}
            </Button>
            <Button type="button" variant="outline" size="sm" onClick={testConnection} disabled={isBusy || !connected}>
              Test
            </Button>
            <Button type="button" variant="outline" size="sm" onClick={disconnect} disabled={isBusy || !connected}>
              <LogOut className="mr-2 h-4 w-4" />
              Disconnect
            </Button>
          </div>
        </div>

        <div className="mt-6 grid gap-4 md:grid-cols-2">
          <div className="space-y-2">
            <Label htmlFor="o365-tenant-id">Tenant ID</Label>
            <Input
              id="o365-tenant-id"
              value={settings.tenantId}
              onChange={(event) => setSettings({ ...settings, tenantId: event.target.value })}
              placeholder="common, organizations, or tenant GUID"
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="o365-client-id">Client ID</Label>
            <Input
              id="o365-client-id"
              value={settings.clientId}
              onChange={(event) => setSettings({ ...settings, clientId: event.target.value })}
              placeholder="Application client ID"
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="o365-redirect-uri">Redirect URI</Label>
            <div className="flex gap-2">
              <Input
                id="o365-redirect-uri"
                value={settings.redirectUri}
                onChange={(event) => setSettings({ ...settings, redirectUri: event.target.value })}
                placeholder={RECOMMENDED_REDIRECT_URI}
              />
              <Button
                type="button"
                variant="outline"
                onClick={() => setSettings({ ...settings, redirectUri: RECOMMENDED_REDIRECT_URI })}
              >
                Use Default
              </Button>
            </div>
            <p className="text-xs text-gray-500 dark:text-gray-400">
              Register this exact URI in Entra under Mobile and desktop applications. Do not paste a Microsoft
              authorize or token endpoint here.
            </p>
          </div>
          <div className="space-y-2">
            <Label htmlFor="o365-scopes">Graph Scopes</Label>
            <Input
              id="o365-scopes"
              value={settings.scopes}
              onChange={(event) => setSettings({ ...settings, scopes: event.target.value })}
              onBlur={() => setSettings((current) => normalizeSettings(current))}
            />
          </div>
        </div>

        {lastAuthUrl && (
          <div className="mt-5 space-y-2">
            <Label htmlFor="o365-auth-url-debug">Generated Authorize URL</Label>
            <Textarea
              id="o365-auth-url-debug"
              readOnly
              value={lastAuthUrl}
              className="h-24 resize-none text-xs text-gray-600 dark:text-gray-300"
            />
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={async () => {
                await navigator.clipboard.writeText(lastAuthUrl);
                toast.success('Authorize URL copied');
              }}
            >
              <Copy className="mr-2 h-4 w-4" />
              Copy Authorize URL
            </Button>
          </div>
        )}

        <div className="mt-5 space-y-2">
          <Label htmlFor="o365-redirect-result">Microsoft Redirect URL</Label>
          <div className="flex gap-2">
            <Input
              id="o365-redirect-result"
              value={redirectUrl}
              onChange={(event) => setRedirectUrl(event.target.value)}
              placeholder="Paste the full redirect URL after browser sign-in"
            />
            <Button type="button" variant="outline" onClick={completeSignIn} disabled={isBusy}>
              Complete
            </Button>
          </div>
        </div>
      </div>

      <div className="rounded-lg border border-gray-200 bg-white p-6 shadow-sm dark:border-gray-800 dark:bg-gray-900">
        <div className="flex items-center justify-between gap-4">
          <div>
            <h3 className="text-lg font-semibold text-gray-900 dark:text-gray-100">Upcoming Events</h3>
            <p className="mt-1 text-sm text-gray-600 dark:text-gray-400">
              Start recording from an event to use its title, attendees, and description.
            </p>
          </div>
          <Button type="button" variant="outline" size="sm" onClick={refreshEvents} disabled={isBusy || !connected}>
            <RefreshCw className="mr-2 h-4 w-4" />
            Refresh
          </Button>
        </div>

        <div className="mt-4 divide-y divide-gray-200 dark:divide-gray-800">
          {events.length === 0 ? (
            <div className="py-8 text-sm text-gray-500 dark:text-gray-400">
              {connected ? 'No events loaded yet.' : 'Connect your Microsoft 365 calendar to load events.'}
            </div>
          ) : (
            events.map((event) => (
              <div key={event.id} className="flex flex-col gap-3 py-4 lg:flex-row lg:items-center lg:justify-between">
                <div className="min-w-0">
                  <div className="flex items-center gap-2 text-sm font-medium text-gray-900 dark:text-gray-100">
                    <CalendarDays className="h-4 w-4 text-blue-600" />
                    <span className="truncate">{event.title}</span>
                  </div>
                  <div className="mt-1 text-xs text-gray-500 dark:text-gray-400">
                    {formatEventTime(event.start)} - {formatEventTime(event.end)}
                  </div>
                  {event.description && (
                    <Textarea
                      readOnly
                      value={event.description}
                      className="mt-2 h-16 resize-none text-xs text-gray-600 dark:text-gray-300"
                    />
                  )}
                </div>
                <div className="flex gap-2">
                  <Button type="button" variant="outline" size="sm" onClick={() => openEvent(event)} disabled={!event.joinUrl && !event.webLink}>
                    <ExternalLink className="mr-2 h-4 w-4" />
                    Open
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    onClick={() => startRecordingFromEvent(event)}
                    className="bg-blue-600 text-white hover:bg-blue-700 dark:bg-blue-500 dark:text-white dark:hover:bg-blue-600"
                  >
                    <Play className="mr-2 h-4 w-4" />
                    Record
                  </Button>
                </div>
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}

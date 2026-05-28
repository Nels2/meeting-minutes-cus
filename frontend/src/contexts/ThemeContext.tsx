'use client';

import React, { createContext, useContext, useEffect, useState } from 'react';
import { setTheme as setTauriTheme } from '@tauri-apps/api/app';
import { getCurrentWindow } from '@tauri-apps/api/window';

export type Theme = 'light' | 'dark' | 'system';

interface ThemeContextValue {
  theme: Theme;
  setTheme: (theme: Theme) => void;
  resolvedTheme: 'light' | 'dark';
}

const ThemeContext = createContext<ThemeContextValue | undefined>(undefined);
const THEME_STORAGE_KEY = 'meetily-theme';

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [theme, setThemeState] = useState<Theme>('system');
  const [systemTheme, setSystemTheme] = useState<'light' | 'dark'>('light');
  const resolvedTheme = theme === 'system' ? systemTheme : theme;

  useEffect(() => {
    const savedTheme = window.localStorage.getItem(THEME_STORAGE_KEY) as Theme | null;
    if (savedTheme && ['light', 'dark', 'system'].includes(savedTheme)) {
      setThemeState(savedTheme);
    }
  }, []);

  useEffect(() => {
    const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');

    const applySystemTheme = async () => {
      let nextSystemTheme: 'light' | 'dark' = mediaQuery.matches ? 'dark' : 'light';

      try {
        const nativeTheme = await getCurrentWindow().theme();
        if (nativeTheme === 'light' || nativeTheme === 'dark') {
          nextSystemTheme = nativeTheme;
        }
      } catch {
        // Browser dev mode does not expose the Tauri window API.
      }

      setSystemTheme(nextSystemTheme);
    };

    let unlistenNativeTheme: (() => void) | undefined;
    applySystemTheme();
    mediaQuery.addEventListener('change', applySystemTheme);
    getCurrentWindow()
      .onThemeChanged(({ payload }) => {
        if (payload === 'light' || payload === 'dark') {
          setSystemTheme(payload);
        }
      })
      .then(unlisten => {
        unlistenNativeTheme = unlisten;
      })
      .catch(() => {
        // Browser dev mode does not expose the Tauri window API.
      });

    return () => {
      mediaQuery.removeEventListener('change', applySystemTheme);
      unlistenNativeTheme?.();
    };
  }, []);

  useEffect(() => {
    document.documentElement.classList.toggle('dark', resolvedTheme === 'dark');

    setTauriTheme(theme === 'system' ? null : theme).catch(() => {
      // Browser dev mode does not expose the Tauri app API.
    });
  }, [resolvedTheme, theme]);

  const setTheme = (nextTheme: Theme) => {
    setThemeState(nextTheme);
    window.localStorage.setItem(THEME_STORAGE_KEY, nextTheme);
  };

  return (
    <ThemeContext.Provider value={{ theme, setTheme, resolvedTheme }}>
      {children}
    </ThemeContext.Provider>
  );
}

export function useTheme() {
  const context = useContext(ThemeContext);
  if (!context) {
    throw new Error('useTheme must be used within a ThemeProvider');
  }

  return context;
}

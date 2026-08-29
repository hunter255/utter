import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event'

import {
  initializeLocale,
  normalizeLocalePreference,
  type Locale,
  type LocalePreference,
} from './i18n'

export type UiLocalePreference = Locale | 'system'

const CACHE_KEY = 'utter.ui.locale'
const CHANGE_EVENT = 'ui-locale-changed'

export function cachedLocalePreference(): UiLocalePreference {
  return normalizeLocalePreference(localStorage.getItem(CACHE_KEY))
}

/** Applies a preference in the current webview and caches it for the next launch. */
export function applyLocalePreference(preference: LocalePreference | string): Locale {
  const normalized = normalizeLocalePreference(preference)
  localStorage.setItem(CACHE_KEY, normalized)
  return initializeLocale(normalized)
}

/** Applies locally first, then tells the other Tauri windows (notably the HUD). */
export async function broadcastLocalePreference(
  preference: LocalePreference | string,
): Promise<void> {
  const normalized = normalizeLocalePreference(preference)
  applyLocalePreference(normalized)
  await emit(CHANGE_EVENT, normalized).catch(() => {})
}

export function listenForLocalePreference(): Promise<UnlistenFn> {
  return listen<string>(CHANGE_EVENT, (event) => applyLocalePreference(event.payload))
}

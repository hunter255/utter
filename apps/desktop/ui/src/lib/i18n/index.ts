import { derived, get, writable, type Readable } from 'svelte/store'

import { en, type MessageKey } from './en'
import { ru } from './ru'

export const SUPPORTED_LOCALES = ['en', 'ru'] as const
export type Locale = (typeof SUPPORTED_LOCALES)[number]
export type LocalePreference = Locale | 'system' | null | undefined
export type MessageValue = string | number
export type MessageValues = Record<string, MessageValue>
export type Translator = (key: MessageKey, values?: MessageValues) => string
export type Catalog = Partial<Record<MessageKey, string>>

const catalogs: Record<Locale, Catalog> = { en, ru }
const localeState = writable<Locale>('en')

export const locale: Readable<Locale> = { subscribe: localeState.subscribe }

function languageRoot(value: string): string {
  return value.trim().toLowerCase().split(/[-_]/, 1)[0]
}

function supportedLocale(value: string): Locale | null {
  const root = languageRoot(value)
  return SUPPORTED_LOCALES.includes(root as Locale) ? (root as Locale) : null
}

/** Normalizes stored/config values without letting an old unknown value break startup. */
export function normalizeLocalePreference(value: unknown): Locale | 'system' {
  if (typeof value !== 'string' || value === 'system') return 'system'
  return supportedLocale(value) ?? 'system'
}

/** Resolves an explicit preference, or the first supported system language. */
export function resolveLocale(
  preference: LocalePreference | string,
  systemLanguages: readonly string[] = [],
): Locale {
  if (preference && preference !== 'system') return supportedLocale(preference) ?? 'en'
  for (const language of systemLanguages) {
    const resolved = supportedLocale(language)
    if (resolved) return resolved
  }
  return 'en'
}

function interpolate(template: string, values: MessageValues): string {
  return template.replace(/\{([A-Za-z0-9_]+)\}/g, (placeholder, name: string) =>
    Object.prototype.hasOwnProperty.call(values, name) ? String(values[name]) : placeholder,
  )
}

/** Looks up one message and always falls back to the English source catalog. */
export function translate(
  key: MessageKey,
  values: MessageValues = {},
  activeLocale: Locale = 'en',
): string {
  const template = catalogs[activeLocale]?.[key]
    ?? (en as Record<string, string>)[key]
    ?? key
  return interpolate(template, values)
}

export function createTranslator(activeLocale: Locale): Translator {
  return (key, values) => translate(key, values, activeLocale)
}

export const t: Readable<Translator> = derived(localeState, createTranslator)

export function localeDirection(_locale: Locale): 'ltr' | 'rtl' {
  return 'ltr'
}

export function applyDocumentLocale(
  activeLocale: Locale,
  documentRoot: Pick<HTMLElement, 'lang' | 'dir'> | null =
    typeof document === 'undefined' ? null : document.documentElement,
): void {
  if (!documentRoot) return
  documentRoot.lang = activeLocale
  documentRoot.dir = localeDirection(activeLocale)
}

export function setLocale(activeLocale: Locale): void {
  localeState.set(activeLocale)
  applyDocumentLocale(activeLocale)
}

/** Called before mounting Svelte so onboarding and the HUD never flash a stale locale. */
export function initializeLocale(
  preference: LocalePreference | string = 'system',
  systemLanguages: readonly string[] =
    typeof navigator === 'undefined' ? [] : navigator.languages,
): Locale {
  const activeLocale = resolveLocale(preference, systemLanguages)
  setLocale(activeLocale)
  return activeLocale
}

export function currentLocale(): Locale {
  return get(localeState)
}

/** Compile-time helper for future catalogs that must match every English key. */
export function defineCatalog(catalog: Record<MessageKey, string>): Record<MessageKey, string> {
  return catalog
}

export {
  formatBytes,
  formatDateTime,
  formatDuration,
  formatNumber,
  formatPercent,
} from './format'
export type { MessageKey } from './en'

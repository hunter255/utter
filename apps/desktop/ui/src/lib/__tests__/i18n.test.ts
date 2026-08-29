import { get } from 'svelte/store'
import { describe, expect, it } from 'vitest'

import {
  applyDocumentLocale,
  createTranslator,
  formatBytes,
  formatDuration,
  formatNumber,
  formatPercent,
  locale,
  normalizeLocalePreference,
  resolveLocale,
  setLocale,
  translate,
  type MessageKey,
} from '../i18n'
import { en } from '../i18n/en'
import { ru } from '../i18n/ru'

function placeholders(message: string): string[] {
  return [...message.matchAll(/\{([A-Za-z0-9_]+)\}/g)].map((match) => match[1]).sort()
}

describe('i18n foundation', () => {
  it('resolves explicit and system locale preferences safely', () => {
    expect(resolveLocale('ru', ['en-US'])).toBe('ru')
    expect(resolveLocale('ru-RU', ['en-US'])).toBe('ru')
    expect(resolveLocale('system', ['de-DE', 'ru-RU', 'en-US'])).toBe('ru')
    expect(resolveLocale(null, ['de-DE', 'en-US'])).toBe('en')
    expect(resolveLocale('de-DE', ['ru-RU'])).toBe('en')
    expect(resolveLocale('en', ['ru-RU'])).toBe('en')
    expect(normalizeLocalePreference('ru-RU')).toBe('ru')
    expect(normalizeLocalePreference('de-DE')).toBe('system')
    expect(normalizeLocalePreference(null)).toBe('system')
  })

  it('translates Russian, falls back to English safely, and interpolates values', () => {
    const russian = createTranslator('ru')
    expect(russian('model.downloadingPercent', { percent: 42 })).toBe('Загрузка: 42%')
    expect(translate('app.windowTitle', { section: 'Models' }, 'en')).toBe('Models — Utter')
    expect(translate('common.cancel', {}, 'de' as never)).toBe('Cancel')
    expect(translate('not.in.catalog' as MessageKey)).toBe('not.in.catalog')
  })

  it('keeps the Russian catalog complete and preserves every interpolation argument', () => {
    expect(Object.keys(ru).sort()).toEqual(Object.keys(en).sort())
    for (const key of Object.keys(en) as MessageKey[]) {
      expect(placeholders(ru[key]), key).toEqual(placeholders(en[key]))
    }
  })

  it('updates subscribers and document language metadata together', () => {
    const root = { lang: '', dir: '' }
    applyDocumentLocale('ru', root)
    expect(root).toEqual({ lang: 'ru', dir: 'ltr' })

    setLocale('ru')
    expect(get(locale)).toBe('ru')
    setLocale('en')
  })

  it('formats numbers, percentages, durations, and bytes for the requested locale', () => {
    expect(formatNumber(1234.5, undefined, 'en')).toBe('1,234.5')
    expect(formatPercent(0.42, 'en')).toBe('42%')
    expect(formatDuration(1500, 'en')).toContain('1.5')
    expect(formatBytes(1536, 'en')).toBe('1.5 KB')
    expect(formatNumber(1234.5, undefined, 'ru')).not.toBe(formatNumber(1234.5, undefined, 'en'))

    setLocale('ru')
    expect(formatNumber(1234.5)).toBe(formatNumber(1234.5, undefined, 'ru'))
    setLocale('en')
  })
})

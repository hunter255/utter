import { get } from 'svelte/store'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const eventMocks = vi.hoisted(() => ({
  emit: vi.fn<(event: string, payload: string) => Promise<void>>(async () => undefined),
  listen: vi.fn<
    (event: string, callback: (event: { payload: string }) => void) => Promise<() => void>
  >(async () => () => undefined),
}))

vi.mock('@tauri-apps/api/event', () => eventMocks)

import { locale, setLocale } from '../i18n'
import {
  applyLocalePreference,
  broadcastLocalePreference,
  cachedLocalePreference,
  listenForLocalePreference,
} from '../locale-sync'

describe('locale synchronization', () => {
  let storage: Map<string, string>

  beforeEach(() => {
    storage = new Map()
    vi.stubGlobal('localStorage', {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
    })
    eventMocks.emit.mockClear()
    eventMocks.listen.mockClear()
    setLocale('en')
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('restores a cached preference and treats unknown legacy values as system', () => {
    expect(cachedLocalePreference()).toBe('system')
    storage.set('utter.ui.locale', 'ru')
    expect(cachedLocalePreference()).toBe('ru')
    storage.set('utter.ui.locale', 'de')
    expect(cachedLocalePreference()).toBe('system')
  })

  it('applies and caches a preference before broadcasting it to the HUD', async () => {
    await broadcastLocalePreference('ru')
    expect(get(locale)).toBe('ru')
    expect(storage.get('utter.ui.locale')).toBe('ru')
    expect(eventMocks.emit).toHaveBeenCalledWith('ui-locale-changed', 'ru')
  })

  it('applies locale changes received from another Tauri window', async () => {
    let handler: ((event: { payload: string }) => void) | undefined
    eventMocks.listen.mockImplementationOnce(async (_event, callback) => {
      handler = callback
      return () => undefined
    })

    await listenForLocalePreference()
    handler?.({ payload: 'ru' })
    expect(get(locale)).toBe('ru')
  })

  it('can apply explicit English over a Russian system locale', () => {
    applyLocalePreference('en')
    expect(get(locale)).toBe('en')
  })
})

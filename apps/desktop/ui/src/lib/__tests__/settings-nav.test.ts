import { describe, expect, it } from 'vitest'

import {
  SETTINGS_NAV,
  resolveSettingsSection,
  settingsWindowTitle,
} from '../settings-nav'

describe('settings navigation', () => {
  it('keeps every grouped destination one flat unique route', () => {
    const routes = SETTINGS_NAV.flatMap((group) => group.items.map((item) => item.hash))
    expect(SETTINGS_NAV.map((group) => group.label)).toEqual([
      'Dictation',
      'Resources',
      'Application',
    ])
    expect(new Set(routes).size).toBe(routes.length)
  })

  it.each([
    ['engines', 'models'],
    ['refinement', 'connections'],
    ['dictionary', 'vocabulary'],
    ['snippets', 'vocabulary'],
  ])('redirects legacy #%s to #%s', (legacy, current) => {
    expect(resolveSettingsSection(legacy)).toBe(current)
  })

  it('restores a valid last section only when no explicit hash exists', () => {
    expect(resolveSettingsSection('', 'models')).toBe('models')
    expect(resolveSettingsSection('', 'engines')).toBe('models')
    expect(resolveSettingsSection('not-a-page', 'models')).toBe('general')
  })

  it('derives a useful native window title from the active destination', () => {
    expect(settingsWindowTitle('vocabulary')).toBe('Vocabulary — Utter')
  })
})

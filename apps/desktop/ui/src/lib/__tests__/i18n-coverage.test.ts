import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

const MIGRATED_VIEWS = [
  '../../App.svelte',
  '../../hud/Hud.svelte',
  '../../pages/Onboarding.svelte',
  '../components/HotkeyPicker.svelte',
  '../components/MacosPermissionRecovery.svelte',
  '../components/ModelInstallAction.svelte',
  '../components/ModelOperationStatus.svelte',
  '../components/Notices.svelte',
  '../components/ProfileCard.svelte',
] as const

function markupFor(path: string): string {
  return readFileSync(fileURLToPath(new URL(path, import.meta.url)), 'utf8')
    .replace(/<script\b[^>]*>[\s\S]*?<\/script>/g, '')
    .replace(/<style\b[^>]*>[\s\S]*?<\/style>/g, '')
}

describe('localized view coverage', () => {
  it.each(MIGRATED_VIEWS)('%s contains no untranslated text nodes', (path) => {
    const directText = [...markupFor(path).matchAll(/>([^<>{}]*[A-Za-z][^<>{}]*)</g)]
      .map((match) => match[1].replace(/\s+/g, ' ').trim())
      .filter((text) => text !== 'Utter')

    expect(directText).toEqual([])
  })

  it.each(MIGRATED_VIEWS)('%s contains no untranslated accessibility labels', (path) => {
    const literalAttributes = [
      ...markupFor(path).matchAll(/\b(?:aria-label|alt|placeholder|title)="([^"]*[A-Za-z][^"]*)"/g),
    ].map((match) => match[1])

    expect(literalAttributes).toEqual([])
  })
})

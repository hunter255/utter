import { translate, type MessageKey, type Translator } from './i18n'

export type SettingsSection =
  | 'profiles'
  | 'history'
  | 'models'
  | 'vocabulary'
  | 'connections'
  | 'settings'

export interface SettingsNavItem {
  hash: SettingsSection
  labelKey: MessageKey
}

export interface SettingsNavGroup {
  labelKey: MessageKey
  items: SettingsNavItem[]
}

/** A flat route remains behind the grouped presentation: there is no second
 * navigation level and every destination is still one click away. */
export const SETTINGS_NAV: SettingsNavGroup[] = [
  {
    labelKey: 'nav.group.dictation',
    items: [
      { hash: 'profiles', labelKey: 'nav.profiles' },
      { hash: 'vocabulary', labelKey: 'nav.vocabulary' },
      { hash: 'history', labelKey: 'nav.history' },
    ],
  },
  {
    labelKey: 'nav.group.resources',
    items: [
      { hash: 'models', labelKey: 'nav.models' },
      { hash: 'connections', labelKey: 'nav.connections' },
    ],
  },
  {
    labelKey: 'nav.group.application',
    items: [{ hash: 'settings', labelKey: 'nav.settings' }],
  },
]

const ITEMS = SETTINGS_NAV.flatMap((group) => group.items)
const SECTIONS = new Set<SettingsSection>(ITEMS.map((item) => item.hash))
const LEGACY_ROUTES: Record<string, SettingsSection> = {
  engines: 'models',
  refinement: 'connections',
  dictionary: 'vocabulary',
  snippets: 'vocabulary',
  general: 'settings',
  dictation: 'settings',
  advanced: 'settings',
}

function knownSection(value: string): SettingsSection | null {
  const mapped = LEGACY_ROUTES[value] ?? value
  return SECTIONS.has(mapped as SettingsSection) ? (mapped as SettingsSection) : null
}

/** Resolves current and pre-PR route hashes. The remembered section is used
 * only when a window opens without a hash; a genuinely unknown link goes to
 * Settings so a typo never inherits unrelated navigation history. */
export function resolveSettingsSection(raw: string, remembered = ''): SettingsSection {
  const normalized = raw.trim().replace(/^#/, '').toLowerCase()
  if (normalized) return knownSection(normalized) ?? 'settings'
  return knownSection(remembered.trim().replace(/^#/, '').toLowerCase()) ?? 'settings'
}

export function settingsSectionLabel(
  section: SettingsSection,
  translator: Translator = translate,
): string {
  return translator(ITEMS.find((item) => item.hash === section)?.labelKey ?? 'nav.settings')
}

export function settingsWindowTitle(
  section: SettingsSection,
  translator: Translator = translate,
): string {
  return translator('app.windowTitle', { section: settingsSectionLabel(section, translator) })
}

export type SettingsSection =
  | 'profiles'
  | 'history'
  | 'models'
  | 'vocabulary'
  | 'connections'
  | 'settings'

export interface SettingsNavItem {
  hash: SettingsSection
  label: string
}

export interface SettingsNavGroup {
  label: string
  items: SettingsNavItem[]
}

/** A flat route remains behind the grouped presentation: there is no second
 * navigation level and every destination is still one click away. */
export const SETTINGS_NAV: SettingsNavGroup[] = [
  {
    label: 'Dictation',
    items: [
      { hash: 'profiles', label: 'Profiles' },
      { hash: 'vocabulary', label: 'Vocabulary' },
      { hash: 'history', label: 'History' },
    ],
  },
  {
    label: 'Resources',
    items: [
      { hash: 'models', label: 'Models' },
      { hash: 'connections', label: 'Connections' },
    ],
  },
  {
    label: 'Application',
    items: [{ hash: 'settings', label: 'Settings' }],
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

export function settingsSectionLabel(section: SettingsSection): string {
  return ITEMS.find((item) => item.hash === section)?.label ?? 'Settings'
}

export function settingsWindowTitle(section: SettingsSection): string {
  return `${settingsSectionLabel(section)} — Utter`
}

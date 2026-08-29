import { hasBaseKey, parseChordTokens } from './hotkey'
import { languageLabel, transcriptionLanguageOptions } from './models'
import type { EngineCfg, LanguageProfile, ModelInfo } from './types'

export type ProfileSource = 'local' | 'cloud'

export interface ProfileReadiness {
  ready: boolean
  issues: string[]
}

export function profileSource(profile: LanguageProfile): ProfileSource {
  return profile.engine.active === 'cloud' ? 'cloud' : 'local'
}

export function finalModel(profile: LanguageProfile, models: ModelInfo[]): ModelInfo | null {
  const id =
    profile.engine.active === 'whisper'
      ? profile.engine.whisper_model
      : profile.engine.active === 'sherpa'
        ? profile.engine.sherpa_model
        : null
  return models.find((model) => model.id === id && model.role === 'final') ?? null
}

export function previewModel(profile: LanguageProfile, models: ModelInfo[]): ModelInfo | null {
  const id = profile.draft?.model
  return models.find((model) => model.id === id && model.role === 'preview') ?? null
}

/** The remembered local choice used when switching a cloud profile back to
 * local. Both engine-specific ids stay in the schema, so no choice is lost. */
export function rememberedLocalModel(profile: LanguageProfile, models: ModelInfo[]): ModelInfo | null {
  if (profile.engine.active !== 'cloud') return finalModel(profile, models)
  const ids = [profile.engine.sherpa_model, profile.engine.whisper_model]
  for (const id of ids) {
    const model = models.find((candidate) => candidate.id === id && candidate.role === 'final')
    if (model) return model
  }
  return null
}

/** Maps one unified local-model picker back onto the unchanged EngineCfg. */
export function engineForLocalModel(profile: LanguageProfile, model: ModelInfo): EngineCfg {
  if (model.role !== 'final' || (model.engine !== 'whisper' && model.engine !== 'sherpa')) {
    return profile.engine
  }
  return model.engine === 'whisper'
    ? { ...profile.engine, active: 'whisper', whisper_model: model.id }
    : { ...profile.engine, active: 'sherpa', sherpa_model: model.id }
}

export function profileLanguageOptions(
  profile: LanguageProfile,
  models: ModelInfo[],
): { value: string; label: string }[] {
  const options = transcriptionLanguageOptions(models)
  const current = profile.language.trim()
  if (current && !options.some((option) => option.value === current)) {
    options.push({ value: current, label: `Custom: ${current}` })
  }
  return options
}

export function profileTitle(profile: LanguageProfile, index: number): string {
  const language = profile.language.trim() ? languageLabel(profile.language) : 'Automatic language'
  return `${language} · Profile ${index + 1}`
}

export function profileSummary(profile: LanguageProfile, models: ModelInfo[]): string {
  const final =
    profile.engine.active === 'cloud'
      ? `Cloud ${profile.engine.cloud.model || 'model not set'}`
      : finalModel(profile, models)?.label ?? 'Final model not selected'
  const preview = profile.draft
    ? `${previewModel(profile, models)?.label ?? 'Preview model missing'} preview`
    : 'Preview off'
  return `${profile.hotkey || 'No hotkey'} · ${final} · ${preview}`
}

export function profileReadiness(
  profile: LanguageProfile,
  models: ModelInfo[],
  requireBaseKey: boolean,
  hasConflict = false,
): ProfileReadiness {
  const issues: string[] = []
  const parsed = parseChordTokens(profile.hotkey)
  if (!parsed || (requireBaseKey && !hasBaseKey(profile.hotkey))) issues.push('Choose a valid hotkey')
  if (hasConflict) issues.push('Resolve the hotkey conflict')

  if (profile.engine.active === 'cloud') {
    if (!profile.engine.cloud.base_url.trim() || !profile.engine.cloud.model.trim()) {
      issues.push('Complete cloud transcription settings')
    }
  } else {
    const model = finalModel(profile, models)
    if (!model) issues.push('Choose a transcription model')
    else if (model.status === 'damaged') issues.push('Re-download the damaged transcription model')
    else if (model.status !== 'ready') issues.push('Download the transcription model')
  }

  if (profile.draft) {
    const model = previewModel(profile, models)
    if (!model) issues.push('Choose a valid preview model')
    else if (model.status === 'damaged') issues.push('Re-download the damaged preview model')
    else if (model.status !== 'ready') issues.push('Download the preview model')
  }

  return { ready: issues.length === 0, issues }
}

export function recognitionSettingsVisible(profile: LanguageProfile): boolean {
  return profile.engine.active !== 'sherpa'
}

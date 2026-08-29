import { hasBaseKey, parseChordTokens } from './hotkey'
import { translate, type MessageKey, type Translator } from './i18n'
import { languageLabel, transcriptionLanguageOptions } from './models'
import type { EngineCfg, LanguageProfile, ModelInfo } from './types'

export type ProfileSource = 'local' | 'cloud'

export interface ProfileReadiness {
  ready: boolean
  issues: MessageKey[]
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
  translator: Translator = translate,
): { value: string; label: string }[] {
  const options = transcriptionLanguageOptions(models, translator)
  const current = profile.language.trim()
  if (current && !options.some((option) => option.value === current)) {
    options.push({
      value: current,
      label: translator('model.customLanguage', { language: current }),
    })
  }
  return options
}

export function profileTitle(
  profile: LanguageProfile,
  index: number,
  translator: Translator = translate,
): string {
  const language = profile.language.trim()
    ? languageLabel(profile.language, translator)
    : translator('profiles.languageAutomatic')
  return translator('profiles.titleFormat', { language, number: index + 1 })
}

export function profileSummary(
  profile: LanguageProfile,
  models: ModelInfo[],
  translator: Translator = translate,
): string {
  const final =
    profile.engine.active === 'cloud'
      ? translator('profiles.summary.cloud', {
          model: profile.engine.cloud.model || translator('profiles.summary.cloudModelMissing'),
        })
      : finalModel(profile, models)?.label ?? translator('profiles.summary.finalMissing')
  const preview = profile.draft
    ? translator('profiles.summary.preview', {
        model: previewModel(profile, models)?.label ?? translator('profiles.summary.previewMissing'),
      })
    : translator('profiles.summary.previewOff')
  return `${profile.hotkey || translator('profiles.summary.noHotkey')} · ${final} · ${preview}`
}

export function profileReadiness(
  profile: LanguageProfile,
  models: ModelInfo[],
  requireBaseKey: boolean,
  hasConflict = false,
): ProfileReadiness {
  const issues: MessageKey[] = []
  const parsed = parseChordTokens(profile.hotkey)
  if (!parsed || (requireBaseKey && !hasBaseKey(profile.hotkey))) {
    issues.push('profiles.issue.validHotkey')
  }
  if (hasConflict) issues.push('profiles.issue.hotkeyConflict')

  if (profile.engine.active === 'cloud') {
    if (!profile.engine.cloud.base_url.trim() || !profile.engine.cloud.model.trim()) {
      issues.push('profiles.issue.cloudSettings')
    }
  } else {
    const model = finalModel(profile, models)
    if (!model) issues.push('profiles.issue.chooseFinalModel')
    else if (model.status === 'damaged') issues.push('profiles.issue.redownloadFinalModel')
    else if (model.status !== 'ready') issues.push('profiles.issue.downloadFinalModel')
  }

  if (profile.draft) {
    const model = previewModel(profile, models)
    if (!model) issues.push('profiles.issue.choosePreviewModel')
    else if (model.status === 'damaged') issues.push('profiles.issue.redownloadPreviewModel')
    else if (model.status !== 'ready') issues.push('profiles.issue.downloadPreviewModel')
  }

  return { ready: issues.length === 0, issues }
}

export function recognitionSettingsVisible(profile: LanguageProfile): boolean {
  return profile.engine.active !== 'sherpa'
}

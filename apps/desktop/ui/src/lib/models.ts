// Model selection helpers driven only by the capability metadata returned by
// `list_models` (`crates/utter-store/src/models.rs`). Model ids never appear
// here: adding a catalog entry should automatically make it available in the
// right selector with the right language warning.

import type { ModelInfo } from './types'

/** Catalog entries that can produce the final transcript inserted into the
 * focused app. Streaming preview entries are intentionally excluded. */
export function transcriptionModels(models: ModelInfo[]): ModelInfo[] {
  return models.filter((m) => m.role === 'final')
}

/** Human-readable language coverage for selectors and model details. */
export function modelLanguageLabel(model: ModelInfo): string {
  if (model.supported_languages.includes('*')) return 'Multilingual'
  return model.supported_languages.map(languageLabel).join(' + ')
}

export function languageLabel(language: string): string {
  const root = language.toLowerCase().split(/[-_]/, 1)[0]
  if (root === 'ru') return 'Russian'
  if (root === 'en') return 'English'
  return language.toUpperCase()
}

/** Languages explicitly represented by final models, in catalog order. The
 * automatic option remains available for multilingual models; adding another
 * single-language catalog entry automatically extends onboarding. */
export function transcriptionLanguageOptions(
  models: ModelInfo[],
): { value: string; label: string }[] {
  const languages: string[] = []
  for (const model of transcriptionModels(models)) {
    for (const language of model.supported_languages) {
      const normalized = language.trim().toLowerCase().split(/[-_]/, 1)[0]
      if (!normalized || normalized === '*' || languages.includes(normalized)) continue
      languages.push(normalized)
    }
  }
  return [
    { value: '', label: 'Automatic (multilingual models)' },
    ...languages.map((language) => ({ value: language, label: languageLabel(language) })),
  ]
}

function performanceLabel(model: ModelInfo): string {
  if (model.performance_class === 'fast') return 'Fast'
  if (model.performance_class === 'balanced') return 'Balanced'
  return 'Heavy'
}

/** Compact, qualitative summary. It deliberately carries no timings because
 * latency varies substantially by CPU/GPU and acceleration backend. */
export function modelCapabilityLabel(model: ModelInfo): string {
  return [
    modelLanguageLabel(model),
    performanceLabel(model),
    ...model.recommendation_tags,
  ]
    .filter(Boolean)
    .join(' · ')
}

/** Whether a profile's explicit BCP-47 hint is compatible with a model.
 * Empty/`auto` is always accepted: it asks the engine to use its own coverage
 * and must not create a false warning for multilingual Whisper. */
export function modelSupportsLanguage(model: ModelInfo, language: string): boolean {
  const normalized = language.trim().toLowerCase()
  if (!normalized || normalized === 'auto' || model.supported_languages.includes('*')) return true
  const root = normalized.split(/[-_]/, 1)[0]
  return model.supported_languages.some((supported) => {
    const supportedRoot = supported.toLowerCase().split(/[-_]/, 1)[0]
    return supportedRoot === root
  })
}

/** Actionable warning rather than a hard block: expert configurations stay
 * possible, but an accidental GigaAM/English or Parakeet/Russian pairing is
 * visible before the user downloads a model. */
export function modelLanguageWarning(model: ModelInfo | null, language: string): string | null {
  if (!model || modelSupportsLanguage(model, language)) return null
  return `${model.label} is designed for ${modelLanguageLabel(model)}, but this profile is set to ${languageLabel(language)}. Change the language or choose another model.`
}

/** Flat options for a picker that spans both local transcription engines.
 * Prefixing the engine keeps similarly named models unambiguous without
 * teaching the shared Select component about groups. */
export function transcriptionModelOptions(
  models: ModelInfo[],
): { value: string; label: string }[] {
  return transcriptionModels(models).map((m) => ({
    value: m.id,
    label: `${m.engine === 'whisper' ? 'Whisper' : 'Sherpa-onnx'} — ${m.label} — ${modelCapabilityLabel(m)} — ${m.size_mb} MB${m.installed ? ' — installed' : ''}`,
  }))
}

/** Every streaming preview model in `models`, in catalog order. */
export function previewModels(models: ModelInfo[]): ModelInfo[] {
  return models.filter((m) => m.role === 'preview')
}

/** The options of a profile's preview-model picker: the preview switched off
 * first (the default — an empty value the caller maps back to a `null`
 * `LanguageProfile.draft`), then one entry per streaming model.
 *
 * A model that is not installed yet is still offered, but says so: selecting
 * one is legal and saves fine, it simply produces no preview until the model
 * is downloaded on the Engines page *and* the app is restarted, since a
 * profile's engines are built once and cached for the run. Naming that up
 * front is what pushes the working order — download first, then select — for
 * a picker that otherwise looks like every option in it is ready to use. */
export function previewModelOptions(models: ModelInfo[]): { value: string; label: string }[] {
  return [
    { value: '', label: 'Off' },
    ...previewModels(models).map((m) => ({
      value: m.id,
      label: `${m.label} — ${modelCapabilityLabel(m)}${m.installed ? '' : ' (not downloaded)'}`,
    })),
  ]
}

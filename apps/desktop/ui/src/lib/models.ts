// Model selection helpers driven only by the capability metadata returned by
// `list_models` (`crates/utter-store/src/models.rs`). Model ids never appear
// here: adding a catalog entry should automatically make it available in the
// right selector with the right language warning.

import { formatBytes, translate, type MessageKey, type Translator } from './i18n'
import type { ModelInfo } from './types'

const RECOMMENDATION_KEYS: Record<string, MessageKey> = {
  'Lowest latency': 'model.recommendation.lowestLatency',
  'Lower accuracy': 'model.recommendation.lowerAccuracy',
  Lightweight: 'model.recommendation.lightweight',
  'Balanced multilingual': 'model.recommendation.balancedMultilingual',
  'Higher accuracy': 'model.recommendation.higherAccuracy',
  'High accuracy': 'model.recommendation.highAccuracy',
  'Mixed language': 'model.recommendation.mixedLanguage',
  'Mixed Russian + English': 'model.recommendation.mixedRussianEnglish',
  'Stable quality': 'model.recommendation.stableQuality',
  'Recommended for Russian': 'model.recommendation.recommendedRussian',
  'Recommended for English': 'model.recommendation.recommendedEnglish',
  'Punctuation included': 'model.recommendation.punctuationIncluded',
  'Live preview only': 'model.recommendation.livePreviewOnly',
  'Accuracy-focused Russian preview': 'model.recommendation.accuracyRussianPreview',
  'No dictionary bias': 'model.recommendation.noDictionaryBias',
  'CPU only': 'model.recommendation.cpuOnly',
  'Lowercase Cyrillic; no punctuation, digits, or Latin':
    'model.recommendation.lowercaseCyrillic',
  'Russian + English code-switching': 'model.recommendation.russianEnglishCodeSwitching',
  'Automatic language detection': 'model.recommendation.automaticLanguageDetection',
}

/** Catalog entries that can produce the final transcript inserted into the
 * focused app. Streaming preview entries are intentionally excluded. */
export function transcriptionModels(models: ModelInfo[]): ModelInfo[] {
  return models.filter((m) => m.role === 'final')
}

/** Human-readable language coverage for selectors and model details. */
export function modelLanguageLabel(model: ModelInfo, translator: Translator = translate): string {
  if (model.supported_languages.includes('*')) return translator('model.language.multilingual')
  return model.supported_languages.map((language) => languageLabel(language, translator)).join(' + ')
}

export function languageLabel(language: string, translator: Translator = translate): string {
  const root = language.toLowerCase().split(/[-_]/, 1)[0]
  if (root === 'ru') return translator('model.language.russian')
  if (root === 'en') return translator('model.language.english')
  return language.toUpperCase()
}

/** Languages explicitly represented by final models, in catalog order. The
 * automatic option remains available for multilingual models; adding another
 * single-language catalog entry automatically extends onboarding. */
export function transcriptionLanguageOptions(
  models: ModelInfo[],
  translator: Translator = translate,
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
    { value: '', label: translator('model.language.automatic') },
    ...languages.map((language) => ({
      value: language,
      label: languageLabel(language, translator),
    })),
  ]
}

function performanceLabel(model: ModelInfo, translator: Translator): string {
  if (model.performance_class === 'fast') return translator('model.performance.fast')
  if (model.performance_class === 'balanced') return translator('model.performance.balanced')
  return translator('model.performance.heavy')
}

function recommendationLabel(tag: string, translator: Translator): string {
  const key = RECOMMENDATION_KEYS[tag]
  return key ? translator(key) : tag
}

/** Compact, qualitative summary. It deliberately carries no timings because
 * latency varies substantially by CPU/GPU and acceleration backend. */
export function modelCapabilityLabel(model: ModelInfo, translator: Translator = translate): string {
  return [
    modelLanguageLabel(model, translator),
    performanceLabel(model, translator),
    ...model.recommendation_tags.map((tag) => recommendationLabel(tag, translator)),
  ]
    .filter(Boolean)
    .join(' · ')
}

export function modelAvailabilityLabel(model: ModelInfo, translator: Translator = translate): string {
  if (model.status === 'ready') return translator('model.availability.installed')
  if (model.status === 'damaged') return translator('model.availability.damaged')
  return translator('model.availability.missing')
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
export function modelLanguageWarning(
  model: ModelInfo | null,
  language: string,
  translator: Translator = translate,
): string | null {
  if (!model || modelSupportsLanguage(model, language)) return null
  return translator('model.languageWarning', {
    model: model.label,
    supported: modelLanguageLabel(model, translator),
    selected: languageLabel(language, translator),
  })
}

/** Flat options for a picker that spans both local transcription engines.
 * Prefixing the engine keeps similarly named models unambiguous without
 * teaching the shared Select component about groups. */
export function transcriptionModelOptions(
  models: ModelInfo[],
  translator: Translator = translate,
): { value: string; label: string }[] {
  return transcriptionModels(models).map((m) => ({
    value: m.id,
    label: `${m.engine === 'whisper' ? 'Whisper' : 'Sherpa-onnx'} — ${m.label} — ${modelCapabilityLabel(m, translator)} — ${formatBytes(m.size_mb * 1024 ** 2)} — ${modelAvailabilityLabel(m, translator)}`,
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
 * A model that is not installed yet is still offered, but says so. The
 * profile page renders `ModelInstallAction` directly below this selector, so
 * the user can finish setup without navigating to the full model library. */
export function previewModelOptions(
  models: ModelInfo[],
  translator: Translator = translate,
): { value: string; label: string }[] {
  return [
    { value: '', label: translator('model.preview.off') },
    ...previewModels(models).map((m) => ({
      value: m.id,
      label: `${m.label} — ${modelCapabilityLabel(m, translator)} (${modelAvailabilityLabel(m, translator)})`,
    })),
  ]
}

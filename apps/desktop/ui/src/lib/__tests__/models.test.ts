import { describe, expect, it } from 'vitest'

import {
  modelCapabilityLabel,
  modelAvailabilityLabel,
  modelLanguageWarning,
  modelSupportsLanguage,
  previewModelOptions,
  previewModels,
  transcriptionModelOptions,
  transcriptionModels,
  transcriptionLanguageOptions,
} from '../models'
import type { ModelInfo } from '../types'

function model(
  id: string,
  engine: string,
  installed: boolean,
  capabilities: Partial<
    Pick<ModelInfo, 'role' | 'supported_languages' | 'performance_class' | 'recommendation_tags'>
  > = {},
): ModelInfo {
  return {
    id,
    engine,
    label: `${id} label`,
    size_mb: 1,
    installed,
    status: installed ? 'ready' : 'missing',
    role: engine === 'sherpa-streaming' ? 'preview' : 'final',
    supported_languages: ['*'],
    performance_class: 'fast',
    recommendation_tags: ['Fixture fit'],
    ...capabilities,
  }
}

/** One entry per engine string the catalog uses (`crates/utter-store/src/models.rs`), so a
 * filter that matched the wrong one has somewhere wrong to land. The streaming entries cover
 * both installed states and every user-visible capability shape from the Rust catalog. */
const CATALOG: ModelInfo[] = [
  model('small', 'whisper', true),
  model('parakeet-tdt-110m-en', 'sherpa', true, { supported_languages: ['en'] }),
  model('zipformer-ru-small', 'sherpa-streaming', true, { supported_languages: ['ru'] }),
  model('t-one-ru', 'sherpa-streaming', true, {
    supported_languages: ['ru'],
    performance_class: 'balanced',
    recommendation_tags: ['Accuracy-focused Russian preview', 'Live preview only'],
  }),
  model('nemotron-3.5-multilingual', 'sherpa-streaming', false, {
    supported_languages: ['*'],
    performance_class: 'heavy',
    recommendation_tags: ['Russian + English code-switching', 'Automatic language detection'],
  }),
  model('zipformer-en-small', 'sherpa-streaming', false, { supported_languages: ['en'] }),
]

describe('previewModels', () => {
  it('selects the streaming models and nothing else', () => {
    // The offline `sherpa` entry is the one this must never return: it is the engine whose text
    // actually gets inserted, and offering it as a preview model (or a preview model as an
    // engine) is exactly what the two distinct engine strings exist to prevent.
    expect(previewModels(CATALOG).map((m) => m.id)).toEqual([
      'zipformer-ru-small',
      't-one-ru',
      'nemotron-3.5-multilingual',
      'zipformer-en-small',
    ])
  })
})

describe('transcriptionModels', () => {
  it('offers both final transcript engines and excludes streaming previews', () => {
    expect(transcriptionModels(CATALOG).map((m) => m.id)).toEqual([
      'small',
      'parakeet-tdt-110m-en',
    ])
  })
})

describe('transcriptionModelOptions', () => {
  it('labels engine, size, and installed state without leaking preview models', () => {
    expect(transcriptionModelOptions(CATALOG)).toEqual([
      {
        value: 'small',
        label: 'Whisper — small label — Multilingual · Fast · Fixture fit — 1 MB — installed',
      },
      {
        value: 'parakeet-tdt-110m-en',
        label:
          'Sherpa-onnx — parakeet-tdt-110m-en label — English · Fast · Fixture fit — 1 MB — installed',
      },
    ])
  })
})

describe('transcriptionLanguageOptions', () => {
  it('derives unique explicit languages from final models and ignores preview-only entries', () => {
    expect(
      transcriptionLanguageOptions([
        model('whisper', 'whisper', true),
        model('giga', 'sherpa', true, { supported_languages: ['ru'] }),
        model('parakeet', 'sherpa', true, { supported_languages: ['en', 'en-US'] }),
        model('preview', 'sherpa-streaming', true, { supported_languages: ['de'] }),
      ]),
    ).toEqual([
      { value: '', label: 'Automatic (multilingual models)' },
      { value: 'ru', label: 'Russian' },
      { value: 'en', label: 'English' },
    ])
  })
})

describe('previewModelOptions', () => {
  it('offers "off" first, then one option per streaming model', () => {
    expect(previewModelOptions(CATALOG)).toEqual([
      { value: '', label: 'Off' },
      {
        value: 'zipformer-ru-small',
        label: 'zipformer-ru-small label — Russian · Fast · Fixture fit (installed)',
      },
      {
        value: 't-one-ru',
        label:
          't-one-ru label — Russian · Balanced · Accuracy-focused Russian preview · Live preview only (installed)',
      },
      {
        value: 'nemotron-3.5-multilingual',
        label:
          'nemotron-3.5-multilingual label — Multilingual · Heavy · Russian + English code-switching · Automatic language detection (not downloaded)',
      },
      {
        value: 'zipformer-en-small',
        label: 'zipformer-en-small label — English · Fast · Fixture fit (not downloaded)',
      },
    ])
  })

  it('marks both installed and uninstalled models explicitly', () => {
    const labels = previewModelOptions(CATALOG).map((o) => o.label)

    expect(labels).toContain(
      'zipformer-en-small label — English · Fast · Fixture fit (not downloaded)',
    )
    expect(labels).toContain(
      'zipformer-ru-small label — Russian · Fast · Fixture fit (installed)',
    )
  })

  it('distinguishes damaged files from a model that was never downloaded', () => {
    const damaged = { ...model('broken', 'whisper', false), status: 'damaged' as const }
    expect(modelAvailabilityLabel(damaged)).toBe('damaged — re-download required')
  })

  it('offers "off" with an empty value even when no streaming model is catalogued', () => {
    // The empty value is what `Profiles.svelte` maps back to a `null` `draft` — the off state
    // has to stay reachable on a catalog with no streaming entries at all, or a profile could
    // never switch its preview back off.
    const options = previewModelOptions([model('small', 'whisper', true)])
    expect(options).toEqual([{ value: '', label: 'Off' }])
  })
})

describe('model language compatibility', () => {
  const russian = model('giga', 'sherpa', true, {
    supported_languages: ['ru'],
    recommendation_tags: ['Recommended for Russian'],
  })
  const english = model('parakeet', 'sherpa', true, {
    supported_languages: ['en'],
    recommendation_tags: ['Recommended for English'],
  })
  const multilingual = model('whisper', 'whisper', true)

  it('accepts BCP-47 variants and rejects a different explicit language', () => {
    expect(modelSupportsLanguage(russian, 'ru-RU')).toBe(true)
    expect(modelSupportsLanguage(russian, 'en')).toBe(false)
    expect(modelSupportsLanguage(english, 'en-US')).toBe(true)
    expect(modelSupportsLanguage(english, 'ru')).toBe(false)
  })

  it('warns for GigaAM/English and Parakeet/Russian without blocking selection', () => {
    expect(modelLanguageWarning(russian, 'en')).toContain(
      'designed for Russian, but this profile is set to English',
    )
    expect(modelLanguageWarning(english, 'ru')).toContain(
      'designed for English, but this profile is set to Russian',
    )
  })

  it('does not warn for multilingual or automatic language paths', () => {
    expect(modelLanguageWarning(multilingual, 'ru')).toBeNull()
    expect(modelLanguageWarning(multilingual, 'auto')).toBeNull()
    expect(modelLanguageWarning(russian, '')).toBeNull()
  })

  it('formats qualitative capabilities without inventing benchmark timings', () => {
    expect(modelCapabilityLabel(russian)).toBe('Russian · Fast · Recommended for Russian')
  })
})

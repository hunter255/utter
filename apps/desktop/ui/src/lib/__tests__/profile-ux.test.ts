import { describe, expect, it } from 'vitest'

import {
  engineForLocalModel,
  profileLanguageOptions,
  profileReadiness,
  profileSource,
  profileSummary,
  profileTitle,
  recognitionSettingsVisible,
  rememberedLocalModel,
} from '../profile-ux'
import { defaultSettings, type ModelInfo } from '../types'

function model(id: string, engine: string, role: 'final' | 'preview', status: ModelInfo['status']): ModelInfo {
  return {
    id,
    engine,
    label: `${id} label`,
    size_mb: 1,
    installed: status === 'ready',
    status,
    supported_languages: engine === 'sherpa' ? ['ru'] : ['*'],
    role,
    performance_class: 'fast',
    recommendation_tags: [],
  }
}

const MODELS = [
  model('small', 'whisper', 'final', 'ready'),
  model('giga', 'sherpa', 'final', 'missing'),
  model('preview', 'sherpa-streaming', 'preview', 'ready'),
]

describe('profile editor derivations', () => {
  it('maps a unified local choice without discarding the other engine settings', () => {
    const profile = defaultSettings().profiles[0]
    profile.engine.sherpa_model = 'giga'
    const mapped = engineForLocalModel(profile, MODELS[0])
    expect(mapped.active).toBe('whisper')
    expect(mapped.whisper_model).toBe('small')
    expect(mapped.sherpa_model).toBe('giga')
    expect(mapped.cloud).toEqual(profile.engine.cloud)
  })

  it('remembers a valid local model while a profile uses cloud', () => {
    const profile = defaultSettings().profiles[0]
    profile.engine.active = 'cloud'
    profile.engine.sherpa_model = 'giga'
    expect(profileSource(profile)).toBe('cloud')
    expect(rememberedLocalModel(profile, MODELS)?.id).toBe('giga')
  })

  it('derives a human title and summary without exposing the technical id', () => {
    const profile = defaultSettings().profiles[0]
    profile.language = 'ru'
    profile.hotkey = 'backquote'
    profile.engine.active = 'whisper'
    expect(profileTitle(profile, 0)).toBe('Russian · Profile 1')
    expect(profileSummary(profile, MODELS)).toContain('backquote · small label · Preview off')
  })

  it('reports every setup issue and becomes ready when required models are available', () => {
    const profile = defaultSettings().profiles[0]
    profile.hotkey = 'backquote'
    profile.engine.active = 'sherpa'
    profile.engine.sherpa_model = 'giga'
    profile.draft = { model: 'preview' }

    expect(profileReadiness(profile, MODELS, true).issues).toEqual([
      'profiles.issue.downloadFinalModel',
    ])
    const readyModels = MODELS.map((candidate) =>
      candidate.id === 'giga' ? { ...candidate, installed: true, status: 'ready' as const } : candidate,
    )
    expect(profileReadiness(profile, readyModels, true)).toEqual({ ready: true, issues: [] })
  })

  it('keeps an existing custom BCP-47 value available and hides recognition for sherpa', () => {
    const profile = defaultSettings().profiles[0]
    profile.language = 'pt-BR'
    profile.engine.active = 'sherpa'
    const options = profileLanguageOptions(profile, MODELS)
    expect(options.at(-1)).toEqual({ value: 'pt-BR', label: 'Custom: pt-BR' })
    expect(recognitionSettingsVisible(profile)).toBe(false)
    profile.engine.active = 'cloud'
    expect(recognitionSettingsVisible(profile)).toBe(true)
  })
})

<script lang="ts">
  import { onMount } from 'svelte'

  import * as api from '../lib/api'
  import Field from '../lib/components/Field.svelte'
  import HotkeyPicker from '../lib/components/HotkeyPicker.svelte'
  import ModelInstallAction from '../lib/components/ModelInstallAction.svelte'
  import ProfileCard from '../lib/components/ProfileCard.svelte'
  import Section from '../lib/components/Section.svelte'
  import Select from '../lib/components/Select.svelte'
  import TextInput from '../lib/components/TextInput.svelte'
  import Toggle from '../lib/components/Toggle.svelte'
  import { chordsConflict, hasBaseKey, parseChordTokens } from '../lib/hotkey'
  import { t } from '../lib/i18n'
  import { modelStore } from '../lib/model-store'
  import { modelLanguageWarning, previewModelOptions, transcriptionModelOptions } from '../lib/models'
  import {
    engineForLocalModel,
    finalModel,
    previewModel,
    profileLanguageOptions,
    profileReadiness,
    profileSource,
    profileSummary,
    profileTitle,
    recognitionSettingsVisible,
    rememberedLocalModel,
    type ProfileSource,
  } from '../lib/profile-ux'
  import { mergeDeep, settingsStore, type DeepPartial } from '../lib/stores'
  import type {
    LanguageProfile,
    PlatformCapabilities,
    RecognitionPromptMode,
    Tone,
  } from '../lib/types'

  interface Props {
    capabilities: PlatformCapabilities
  }

  let { capabilities }: Props = $props()

  // App.svelte only mounts pages once `$settingsStore` has finished loading.
  let settings = $derived($settingsStore!)
  let requiresBaseKey = $derived(!capabilities.modifier_only_hotkeys)
  let models = $derived($modelStore.models)
  let modelsError = $derived($modelStore.error)
  let expandedIndex = $state(0)
  let refineKeyConfigured = $state(false)

  let sourceOptions = $derived<{ value: ProfileSource; label: string }[]>([
    { value: 'local', label: $t('profiles.source.local') },
    { value: 'cloud', label: $t('profiles.source.cloud') },
  ])

  let toneOptions = $derived<{ value: Tone; label: string }[]>([
    { value: 'verbatim', label: $t('profiles.tone.verbatim') },
    { value: 'clean', label: $t('profiles.tone.clean') },
    { value: 'formal', label: $t('profiles.tone.formal') },
    { value: 'notes', label: $t('profiles.tone.notes') },
    { value: 'code_comment', label: $t('profiles.tone.codeComment') },
  ])

  let recognitionPromptOptions = $derived<
    { value: RecognitionPromptMode; label: string }[]
  >([
    { value: 'recommended', label: $t('profiles.recognition.recommended') },
    { value: 'disabled', label: $t('profiles.recognition.disabled') },
    { value: 'custom', label: $t('profiles.recognition.custom') },
  ])

  const CLOUD_MODEL_PRESETS = [
    { value: 'gpt-4o-mini-transcribe', label: 'GPT-4o mini' },
    { value: 'gpt-4o-transcribe', label: 'GPT-4o' },
    { value: 'whisper-1', label: 'Whisper 1' },
  ]

  let localModelOptions = $derived([
    { value: '', label: $t('profiles.chooseLocalModel') },
    ...transcriptionModelOptions(models, $t),
  ])
  let previewOptions = $derived(previewModelOptions(models, $t))

  // The UI uses the same chord rules as the Rust hotkey registry. Invalid
  // chords do not participate in conflict detection because the backend
  // would not register them either.
  let invalidHotkeys = $derived(
    settings.profiles.map((profile) => {
      const parsed = parseChordTokens(profile.hotkey)
      return parsed === null || (requiresBaseKey && !hasBaseKey(profile.hotkey))
    }),
  )
  let parsedHotkeys = $derived(
    settings.profiles.map((profile, index) =>
      invalidHotkeys[index] ? null : parseChordTokens(profile.hotkey),
    ),
  )
  let conflictsByIndex = $derived.by(() => {
    const map = new Map<number, string[]>()
    for (let i = 0; i < parsedHotkeys.length; i++) {
      for (let j = i + 1; j < parsedHotkeys.length; j++) {
        const a = parsedHotkeys[i]
        const b = parsedHotkeys[j]
        if (!a || !b || !chordsConflict(a, b)) continue
        map.set(i, [...(map.get(i) ?? []), settings.profiles[j].id])
        map.set(j, [...(map.get(j) ?? []), settings.profiles[i].id])
      }
    }
    return map
  })

  onMount(async () => {
    try {
      refineKeyConfigured = await api.hasApiKey('refine')
    } catch {
      refineKeyConfigured = false
    }
  })

  function updateProfile(index: number, changes: DeepPartial<LanguageProfile>) {
    const profiles = settings.profiles.map((profile, i) =>
      i === index ? mergeDeep(profile, changes) : profile,
    )
    settingsStore.patch({ profiles })
  }

  function selectSource(index: number, profile: LanguageProfile, source: ProfileSource) {
    if (source === 'cloud') {
      updateProfile(index, { engine: { active: 'cloud' } })
      return
    }
    const model = rememberedLocalModel(profile, models)
    updateProfile(index, {
      engine: model ? engineForLocalModel(profile, model) : { active: 'sherpa' },
    })
  }

  function selectLocalModel(index: number, profile: LanguageProfile, id: string) {
    const model = models.find((candidate) => candidate.id === id && candidate.role === 'final')
    if (model) updateProfile(index, { engine: engineForLocalModel(profile, model) })
  }

  function activeLanguageWarning(profile: LanguageProfile): string | null {
    return modelLanguageWarning(finalModel(profile, models), profile.language, $t)
  }

  function draftLanguageWarning(profile: LanguageProfile): string | null {
    return modelLanguageWarning(previewModel(profile, models), profile.language, $t)
  }

  function nextProfileId(): string {
    const existing = new Set(settings.profiles.map((profile) => profile.id))
    let n = settings.profiles.length + 1
    while (existing.has(`profile-${n}`)) n += 1
    return `profile-${n}`
  }

  function addProfile() {
    const nextIndex = settings.profiles.length
    const newProfile: LanguageProfile = {
      id: nextProfileId(),
      hotkey: '',
      language: '',
      engine: {
        active: 'sherpa',
        whisper_model: 'small',
        sherpa_model: null,
        cloud: { base_url: 'https://api.openai.com/v1', model: 'whisper-1' },
      },
      draft: null,
      recognition: { prompt_mode: 'recommended', custom_prompt: '' },
      refine: { enabled: false, tone: 'clean', instructions: '' },
    }
    settingsStore.patch({ profiles: [...settings.profiles, newProfile] })
    expandedIndex = nextIndex
  }

  function removeProfile(index: number) {
    if (settings.profiles.length <= 1) return
    settingsStore.patch({ profiles: settings.profiles.filter((_, i) => i !== index) })
    expandedIndex = Math.min(Math.max(0, index - 1), settings.profiles.length - 2)
  }

  function refinementConnectionState(): { ready: boolean; label: string } {
    if (!settings.refine.enabled) {
      return { ready: false, label: $t('profiles.refinementPaused') }
    }
    const localProvider = /^https?:\/\/(localhost|127\.0\.0\.1)(:\d+)?(\/|$)/i.test(
      settings.refine.base_url,
    )
    return localProvider || refineKeyConfigured
      ? { ready: true, label: $t('profiles.connectionReady') }
      : { ready: false, label: $t('profiles.connectionNeedsSetup') }
  }
</script>

<header class="page-heading">
  <h1>{$t('profiles.title')}</h1>
  <p>{$t('profiles.description')}</p>
</header>

{#if modelsError}
  <p class="error">{$t('profiles.catalogUnavailable', { error: modelsError })}</p>
{/if}

<div class="profile-list">
  {#each settings.profiles as profile, index (index)}
    {@const readiness = profileReadiness(
      profile,
      models,
      requiresBaseKey,
      conflictsByIndex.has(index),
    )}
    <ProfileCard
      title={profileTitle(profile, index, $t)}
      summary={profileSummary(profile, models, $t)}
      ready={readiness.ready}
      expanded={expandedIndex === index}
      onToggle={() => (expandedIndex = expandedIndex === index ? -1 : index)}
    >
      {#if !readiness.ready}
        <div class="setup-needed" role="status">
          <strong>{$t('profiles.finishSetup')}</strong>
          <ul>
            {#each readiness.issues as issue (issue)}
              <li>{$t(issue)}</li>
            {/each}
          </ul>
        </div>
      {/if}

      <section class="group">
        <header class="group-heading">
          <h2>{$t('profiles.group.basics')}</h2>
          <p>{$t('profiles.group.basicsDescription')}</p>
        </header>

        <Field label={$t('profiles.language')} for="profile-{index}-language">
          <Select
            id="profile-{index}-language"
            options={profileLanguageOptions(profile, models, $t)}
            bind:value={
              () => profile.language,
              (value) => updateProfile(index, { language: value })
            }
          />
        </Field>

        <Field
          label={$t('profiles.hotkey')}
          for="profile-{index}-hotkey"
          hint={requiresBaseKey
            ? $t('profiles.hotkeyHint.macos')
            : $t('profiles.hotkeyHint.other')}
        >
          <HotkeyPicker
            id="profile-{index}-hotkey"
            requireBaseKey={requiresBaseKey}
            bind:value={
              () => profile.hotkey,
              (value) => updateProfile(index, { hotkey: value })
            }
          />
          {#if invalidHotkeys[index]}
            <p class="warning">
              {requiresBaseKey
                ? $t('profiles.hotkeyInvalid.macos')
                : $t('profiles.hotkeyInvalid.other')}
            </p>
          {/if}
          {#if conflictsByIndex.has(index)}
            <p class="warning">
              {$t('profiles.hotkeyConflict', {
                profiles: conflictsByIndex.get(index)?.join(', ') ?? '',
              })}
            </p>
          {/if}
        </Field>
      </section>

      <section class="group">
        <header class="group-heading">
          <h2>{$t('profiles.group.transcription')}</h2>
          <p>{$t('profiles.group.transcriptionDescription')}</p>
        </header>

        <Field label={$t('profiles.source')} for="profile-{index}-source">
          <Select
            id="profile-{index}-source"
            options={sourceOptions}
            bind:value={
              () => profileSource(profile),
              (value) => selectSource(index, profile, value as ProfileSource)
            }
          />
        </Field>

        {#if profileSource(profile) === 'local'}
          <Field
            label={$t('profiles.model')}
            for="profile-{index}-local-model"
            hint={$t('profiles.modelHint')}
          >
            <Select
              id="profile-{index}-local-model"
              options={localModelOptions}
              bind:value={
                () => rememberedLocalModel(profile, models)?.id ?? '',
                (value) => selectLocalModel(index, profile, value)
              }
            />
            <ModelInstallAction
              model={finalModel(profile, models)}
              beforeInstall={() => settingsStore.flush()}
            />
            {#if activeLanguageWarning(profile)}
              <p class="warning">{activeLanguageWarning(profile)}</p>
            {/if}
          </Field>
        {:else}
          <Field label={$t('profiles.baseUrl')} for="profile-{index}-cloud-url">
            <TextInput
              id="profile-{index}-cloud-url"
              type="url"
              bind:value={
                () => profile.engine.cloud.base_url,
                (value) => updateProfile(index, { engine: { cloud: { base_url: value } } })
              }
            />
          </Field>
          <Field
            label={$t('profiles.cloudModel')}
            for="profile-{index}-cloud-model"
            hint={$t('profiles.cloudModelHint')}
          >
            <TextInput
              id="profile-{index}-cloud-model"
              bind:value={
                () => profile.engine.cloud.model,
                (value) => updateProfile(index, { engine: { cloud: { model: value } } })
              }
            />
            <div class="model-presets" aria-label={$t('profiles.cloudModelPresets')}>
              {#each CLOUD_MODEL_PRESETS as preset (preset.value)}
                <button
                  type="button"
                  class:active={profile.engine.cloud.model === preset.value}
                  aria-pressed={profile.engine.cloud.model === preset.value}
                  onclick={() =>
                    updateProfile(index, { engine: { cloud: { model: preset.value } } })}
                >{preset.label}</button>
              {/each}
            </div>
          </Field>
        {/if}
      </section>

      <section class="group">
        <header class="group-heading">
          <h2>{$t('profiles.group.preview')}</h2>
          <p>{$t('profiles.group.previewDescription')}</p>
        </header>

        <Field label={$t('profiles.previewModel')} for="profile-{index}-preview">
          <Select
            id="profile-{index}-preview"
            options={previewOptions}
            bind:value={
              () => profile.draft?.model ?? '',
              (value) => updateProfile(index, { draft: value ? { model: value } : null })
            }
          />
          <ModelInstallAction
            model={previewModel(profile, models)}
            beforeInstall={() => settingsStore.flush()}
          />
          {#if draftLanguageWarning(profile)}
            <p class="warning">{draftLanguageWarning(profile)}</p>
          {/if}
        </Field>
      </section>

      <section class="group">
        <header class="group-heading">
          <h2>{$t('profiles.group.refinement')}</h2>
          <p>{$t('profiles.group.refinementDescription')}</p>
        </header>

        <Field label={$t('profiles.refineTranscript')} for="profile-{index}-refine">
          <Toggle
            id="profile-{index}-refine"
            bind:checked={
              () => profile.refine.enabled,
              (value) => updateProfile(index, { refine: { enabled: value } })
            }
          />
        </Field>

        {#if profile.refine.enabled}
          <div
            class:connection-ready={refinementConnectionState().ready}
            class="connection-state"
          >
            <span>{refinementConnectionState().label}</span>
            <a href="#connections">{$t('profiles.openConnections')}</a>
          </div>
          <Field label={$t('profiles.tone')} for="profile-{index}-tone">
            <Select
              id="profile-{index}-tone"
              options={toneOptions}
              bind:value={
                () => profile.refine.tone,
                (value) => updateProfile(index, { refine: { tone: value as Tone } })
              }
            />
          </Field>
          <Field
            label={$t('profiles.instructions')}
            for="profile-{index}-refine-instructions"
            hint={$t('profiles.instructionsHint')}
          >
            <textarea
              id="profile-{index}-refine-instructions"
              rows="4"
              bind:value={
                () => profile.refine.instructions,
                (value) => updateProfile(index, { refine: { instructions: value } })
              }
            ></textarea>
          </Field>
        {/if}
      </section>

      <details class="advanced-settings">
        <summary>{$t('profiles.advanced')}</summary>
        <div class="advanced-body">
          <Field
            label={$t('profiles.technicalId')}
            for="profile-{index}-id"
            hint={$t('profiles.technicalIdHint')}
          >
            <TextInput
              id="profile-{index}-id"
              bind:value={
                () => profile.id,
                (value) => updateProfile(index, { id: value })
              }
            />
          </Field>

          <Field
            label={$t('profiles.customLanguageCode')}
            for="profile-{index}-custom-language"
            hint={$t('profiles.customLanguageCodeHint')}
          >
            <TextInput
              id="profile-{index}-custom-language"
              placeholder="auto"
              bind:value={
                () => profile.language,
                (value) => updateProfile(index, { language: value.trim() })
              }
            />
          </Field>

          {#if recognitionSettingsVisible(profile)}
            <Field
              label={$t('profiles.recognitionPrompt')}
              for="profile-{index}-recognition-prompt-mode"
              hint={$t('profiles.recognitionPromptHint')}
            >
              <Select
                id="profile-{index}-recognition-prompt-mode"
                options={recognitionPromptOptions}
                bind:value={
                  () => profile.recognition.prompt_mode,
                  (value) =>
                    updateProfile(index, {
                      recognition: { prompt_mode: value as RecognitionPromptMode },
                    })
                }
              />
            </Field>
            {#if profile.recognition.prompt_mode === 'custom'}
              <Field
                label={$t('profiles.customRecognitionPrompt')}
                for="profile-{index}-recognition-prompt"
                hint={$t('profiles.customRecognitionPromptHint')}
              >
                <textarea
                  id="profile-{index}-recognition-prompt"
                  rows="4"
                  bind:value={
                    () => profile.recognition.custom_prompt,
                    (value) => updateProfile(index, { recognition: { custom_prompt: value } })
                  }
                ></textarea>
              </Field>
            {/if}
          {/if}
        </div>
      </details>

      <div class="profile-actions">
        <button
          type="button"
          class="remove"
          onclick={() => removeProfile(index)}
          disabled={settings.profiles.length <= 1}
          title={settings.profiles.length <= 1 ? $t('profiles.atLeastOne') : undefined}
        >
          {$t('profiles.remove')}
        </button>
      </div>
    </ProfileCard>
  {/each}
</div>

<Section
  title={$t('profiles.add.title')}
  description={$t('profiles.add.description')}
>
  <div class="profile-actions">
    <button type="button" class="primary" onclick={addProfile}>{$t('profiles.add.action')}</button>
  </div>
</Section>

<style>
  .page-heading {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .page-heading h1 {
    font-size: 20px;
    font-weight: 700;
  }

  .page-heading p,
  .group-heading p {
    color: var(--text-muted);
    font-size: 13px;
  }

  .profile-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding-top: var(--space-4);
  }

  .group + .group {
    border-top: 1px solid var(--border);
  }

  .group-heading {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .group-heading h2 {
    font-size: 14px;
    font-weight: 650;
  }

  .error,
  .warning {
    border-radius: var(--radius-sm);
    font-size: 12px;
  }

  .error {
    padding: var(--space-2) var(--space-3);
    color: var(--danger);
    background: var(--danger-bg);
  }

  .warning {
    padding: var(--space-2) var(--space-3);
    color: var(--warning-text);
    background: var(--warning-bg);
  }

  .setup-needed {
    margin-top: var(--space-4);
    padding: var(--space-3);
    border-radius: var(--radius-sm);
    color: var(--warning-text);
    background: var(--warning-bg);
    font-size: 12px;
  }

  .setup-needed ul {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: var(--space-1) 0 0;
    padding-left: 18px;
  }

  .connection-state {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-sm);
    color: var(--warning-text);
    background: var(--warning-bg);
    font-size: 12px;
  }

  .connection-state.connection-ready {
    color: var(--success);
    background: var(--bg-sunken);
  }

  .connection-state a {
    color: inherit;
    font-weight: 600;
  }

  .advanced-settings {
    border-top: 1px solid var(--border);
    padding-top: var(--space-4);
  }

  .advanced-settings summary {
    color: var(--text-muted);
    cursor: pointer;
    font-size: 13px;
    font-weight: 600;
  }

  .advanced-settings[open] summary {
    color: var(--text);
  }

  .advanced-body {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding-top: var(--space-3);
  }

  .model-presets,
  .profile-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }

  button {
    padding: 5px var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text);
    background: var(--bg-elevated);
    cursor: pointer;
    font-size: 13px;
  }

  button:hover:not(:disabled) {
    background: var(--surface-hover);
  }

  button:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  button.primary {
    border-color: var(--accent);
    color: var(--accent-contrast);
    background: var(--accent);
  }

  button.remove {
    color: var(--danger);
    border-color: var(--danger);
    background: var(--bg);
  }

  button.remove:disabled {
    color: var(--text-muted);
    border-color: var(--border);
    background: var(--bg-elevated);
  }

  .model-presets button {
    color: var(--text-muted);
    font-size: 12px;
  }

  .model-presets button.active {
    color: var(--text);
    border-color: var(--accent);
    background: var(--bg-sunken);
  }

  textarea {
    width: min(100%, 520px);
    min-height: 88px;
    resize: vertical;
    padding: var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--text);
    font: inherit;
    font-size: 13px;
  }
</style>

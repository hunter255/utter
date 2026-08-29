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

  const SOURCE_OPTIONS: { value: ProfileSource; label: string }[] = [
    { value: 'local', label: 'Local — private and offline' },
    { value: 'cloud', label: 'Cloud — OpenAI-compatible' },
  ]

  const TONE_OPTIONS: { value: Tone; label: string }[] = [
    { value: 'verbatim', label: 'Verbatim (no changes)' },
    { value: 'clean', label: 'Clean (punctuation, casing)' },
    { value: 'formal', label: 'Formal' },
    { value: 'notes', label: 'Notes (terse, bulleted)' },
    { value: 'code_comment', label: 'Code comment' },
  ]

  const RECOGNITION_PROMPT_OPTIONS: { value: RecognitionPromptMode; label: string }[] = [
    { value: 'recommended', label: 'Recommended for model' },
    { value: 'disabled', label: 'Off' },
    { value: 'custom', label: 'Custom' },
  ]

  const CLOUD_MODEL_PRESETS = [
    { value: 'gpt-4o-mini-transcribe', label: 'GPT-4o mini' },
    { value: 'gpt-4o-transcribe', label: 'GPT-4o' },
    { value: 'whisper-1', label: 'Whisper 1' },
  ]

  let localModelOptions = $derived([
    { value: '', label: 'Choose a local model…' },
    ...transcriptionModelOptions(models),
  ])
  let previewOptions = $derived(previewModelOptions(models))

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
    return modelLanguageWarning(finalModel(profile, models), profile.language)
  }

  function draftLanguageWarning(profile: LanguageProfile): string | null {
    return modelLanguageWarning(previewModel(profile, models), profile.language)
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
      return { ready: false, label: 'Refinement is paused globally' }
    }
    const localProvider = /^https?:\/\/(localhost|127\.0\.0\.1)(:\d+)?(\/|$)/i.test(
      settings.refine.base_url,
    )
    return localProvider || refineKeyConfigured
      ? { ready: true, label: 'Connection is ready' }
      : { ready: false, label: 'Connection needs setup' }
  }
</script>

<header class="page-heading">
  <h1>Language profiles</h1>
  <p>One hotkey and one complete dictation setup for every language you use.</p>
</header>

{#if modelsError}
  <p class="error">Model catalog unavailable: {modelsError}</p>
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
      title={profileTitle(profile, index)}
      summary={profileSummary(profile, models)}
      ready={readiness.ready}
      expanded={expandedIndex === index}
      onToggle={() => (expandedIndex = expandedIndex === index ? -1 : index)}
    >
      {#if !readiness.ready}
        <div class="setup-needed" role="status">
          <strong>Finish setup</strong>
          <ul>
            {#each readiness.issues as issue (issue)}
              <li>{issue}</li>
            {/each}
          </ul>
        </div>
      {/if}

      <section class="group">
        <header class="group-heading">
          <h2>Basics</h2>
          <p>Choose the language and the key that starts this profile.</p>
        </header>

        <Field label="Language" for="profile-{index}-language">
          <Select
            id="profile-{index}-language"
            options={profileLanguageOptions(profile, models)}
            bind:value={
              () => profile.language,
              (value) => updateProfile(index, { language: value })
            }
          />
        </Field>

        <Field
          label="Hotkey"
          for="profile-{index}-hotkey"
          hint={requiresBaseKey
            ? 'Press any base key, optionally with modifiers. `, Insert, and function keys are supported.'
            : 'Press a key chord; modifier-only shortcuts are supported on this platform.'}
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
                ? 'Add a letter, number, function key, Space, `, or Insert.'
                : 'Choose a valid hotkey for this profile.'}
            </p>
          {/if}
          {#if conflictsByIndex.has(index)}
            <p class="warning">Already used by {conflictsByIndex.get(index)?.join(', ')}.</p>
          {/if}
        </Field>
      </section>

      <section class="group">
        <header class="group-heading">
          <h2>Transcription</h2>
          <p>The model that produces the final text inserted into the active app.</p>
        </header>

        <Field label="Source" for="profile-{index}-source">
          <Select
            id="profile-{index}-source"
            options={SOURCE_OPTIONS}
            bind:value={
              () => profileSource(profile),
              (value) => selectSource(index, profile, value as ProfileSource)
            }
          />
        </Field>

        {#if profileSource(profile) === 'local'}
          <Field
            label="Model"
            for="profile-{index}-local-model"
            hint="Whisper and Sherpa models are shown together; choosing one selects its engine automatically."
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
          <Field label="Base URL" for="profile-{index}-cloud-url">
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
            label="Cloud model"
            for="profile-{index}-cloud-model"
            hint="Choose a preset or enter any model supported by the endpoint."
          >
            <TextInput
              id="profile-{index}-cloud-model"
              bind:value={
                () => profile.engine.cloud.model,
                (value) => updateProfile(index, { engine: { cloud: { model: value } } })
              }
            />
            <div class="model-presets" aria-label="Cloud model presets">
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
          <h2>Live preview</h2>
          <p>Optional provisional text in the HUD while you speak. Final text still uses the model above.</p>
        </header>

        <Field label="Preview model" for="profile-{index}-preview">
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
          <h2>Refinement</h2>
          <p>Optionally polish this profile’s final transcript with an LLM.</p>
        </header>

        <Field label="Refine transcript" for="profile-{index}-refine">
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
            <a href="#connections">Open connection settings</a>
          </div>
          <Field label="Tone" for="profile-{index}-tone">
            <Select
              id="profile-{index}-tone"
              options={TONE_OPTIONS}
              bind:value={
                () => profile.refine.tone,
                (value) => updateProfile(index, { refine: { tone: value as Tone } })
              }
            />
          </Field>
          <Field
            label="Instructions"
            for="profile-{index}-refine-instructions"
            hint="Optional formatting or editing preferences; meaning and language are preserved."
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
        <summary>Advanced profile settings</summary>
        <div class="advanced-body">
          <Field
            label="Technical ID"
            for="profile-{index}-id"
            hint="Used in config, history, and diagnostics. It does not need to match the language."
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
            label="Custom language code"
            for="profile-{index}-custom-language"
            hint="BCP-47 code such as de, fr-CA, or uk. Leave blank for automatic detection."
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
              label="Recognition prompt"
              for="profile-{index}-recognition-prompt-mode"
              hint="Guides recognition before the optional LLM refinement step."
            >
              <Select
                id="profile-{index}-recognition-prompt-mode"
                options={RECOGNITION_PROMPT_OPTIONS}
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
                label="Custom recognition prompt"
                for="profile-{index}-recognition-prompt"
                hint="Keep it concise: spellings, language mix, and punctuation examples work best."
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
          title={settings.profiles.length <= 1 ? 'At least one profile is required' : undefined}
        >
          Remove profile
        </button>
      </div>
    </ProfileCard>
  {/each}
</div>

<Section
  title="Add another language"
  description="Create a separate hotkey, transcription model, preview, and refinement policy."
>
  <div class="profile-actions">
    <button type="button" class="primary" onclick={addProfile}>Add profile</button>
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

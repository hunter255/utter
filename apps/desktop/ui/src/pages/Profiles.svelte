<script lang="ts">
  import { onMount } from 'svelte'

  import Section from '../lib/components/Section.svelte'
  import Field from '../lib/components/Field.svelte'
  import Select from '../lib/components/Select.svelte'
  import TextInput from '../lib/components/TextInput.svelte'
  import Toggle from '../lib/components/Toggle.svelte'
  import HotkeyPicker from '../lib/components/HotkeyPicker.svelte'
  import * as api from '../lib/api'
  import { chordsConflict, hasBaseKey, parseChordTokens } from '../lib/hotkey'
  import { previewModelOptions } from '../lib/models'
  import { mergeDeep, settingsStore, type DeepPartial } from '../lib/stores'
  import type {
    EngineKind,
    LanguageProfile,
    ModelInfo,
    PlatformCapabilities,
    RecognitionPromptMode,
    Tone,
  } from '../lib/types'

  interface Props {
    capabilities: PlatformCapabilities
  }

  let { capabilities }: Props = $props()

  // App.svelte only mounts pages once `$settingsStore` has finished loading,
  // so this non-null assertion is safe for the component's whole lifetime.
  let settings = $derived($settingsStore!)
  let requiresBaseKey = $derived(!capabilities.modifier_only_hotkeys)

  const ENGINE_OPTIONS: { value: EngineKind; label: string }[] = [
    { value: 'whisper', label: 'Whisper (local)' },
    { value: 'sherpa', label: 'Sherpa-onnx (local)' },
    { value: 'cloud', label: 'Cloud (OpenAI-compatible)' },
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

  let models = $state<ModelInfo[]>([])
  let modelsError = $state('')

  let whisperOptions = $derived(
    models.filter((m) => m.engine === 'whisper').map((m) => ({ value: m.id, label: m.label })),
  )
  let sherpaOptions = $derived([
    { value: '', label: 'None selected' },
    ...models.filter((m) => m.engine === 'sherpa').map((m) => ({ value: m.id, label: m.label })),
  ])
  // Streaming models only, never the engine models above — see
  // `previewModelOptions`, which is where that separation is pinned.
  let previewOptions = $derived(previewModelOptions(models))

  onMount(async () => {
    try {
      models = await api.listModels()
    } catch (err) {
      modelsError = String(err)
    }
  })

  // Every profile's hotkey, parsed into the token set `chordsConflict`
  // compares — `null` for a chord that would fail to parse on the Rust side
  // too (see `parseChordTokens`), so it takes part in no conflict here
  // either, mirroring `parse_profile_hotkeys` dropping such a profile from
  // hotkey registration instead of reporting a conflict for it.
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

  // Maps a profile's index to the ids of every other profile whose chord
  // conflicts with it, using `chordsConflict` — a deliberately close mirror
  // of `utter_inject::hotkey::find_conflicts`'s own pairwise scan, so the two
  // stay easy to compare if Rust's rule ever changes. See `../lib/hotkey.ts`
  // for why the two are expected to agree rather than reimplementing the
  // same idea independently.
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

  function updateProfile(index: number, changes: DeepPartial<LanguageProfile>) {
    const profiles = settings.profiles.map((profile, i) =>
      i === index ? mergeDeep(profile, changes) : profile,
    )
    settingsStore.patch({ profiles })
  }

  function nextProfileId(): string {
    const existing = new Set(settings.profiles.map((p) => p.id))
    let n = settings.profiles.length + 1
    while (existing.has(`profile-${n}`)) n += 1
    return `profile-${n}`
  }

  function addProfile() {
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
  }

  // `ProfileRegistry::new` (crates/apps/desktop/src-tauri/src/profiles.rs)
  // only *warns* that dictation has no hotkey once the profile list is
  // empty — it does not refuse the state. A hand-edited `profiles = []` is
  // the only way that happens today (`predates_profiles` only triggers
  // migration when the `profiles` key is absent, not when it's empty), and
  // this UI has no such loophole: disabling Remove below the last profile
  // means it can never construct that state in the first place, rather than
  // constructing it and then explaining the warning afterwards.
  function removeProfile(index: number) {
    if (settings.profiles.length <= 1) return
    settingsStore.patch({ profiles: settings.profiles.filter((_, i) => i !== index) })
  }
</script>

{#if modelsError}
  <p class="error">{modelsError}</p>
{/if}

{#each settings.profiles as profile, index (index)}
  <Section
    title={profile.id || `Profile ${index + 1}`}
    description="A hotkey, an engine and model, and a refinement policy for one language."
  >
    <Field label="ID" for="profile-{index}-id" hint="Used in config, history, and the HUD.">
      <TextInput
        id="profile-{index}-id"
        bind:value={
          () => profile.id,
          (v) => updateProfile(index, { id: v })
        }
      />
    </Field>

    <Field
      label="Language"
      for="profile-{index}-language"
      hint="BCP-47 tag passed to the engine as a transcription hint, e.g. en, ru."
    >
      <TextInput
        id="profile-{index}-language"
        bind:value={
          () => profile.language,
          (v) => updateProfile(index, { language: v })
        }
      />
    </Field>

    <Field
      label="Hotkey"
      for="profile-{index}-hotkey"
      hint={requiresBaseKey
        ? 'macOS requires modifiers plus one regular key, e.g. ctrl+alt+space.'
        : 'Modifiers plus one key, e.g. ctrl+alt+d, or modifiers alone.'}
    >
      <HotkeyPicker
        id="profile-{index}-hotkey"
        requireBaseKey={requiresBaseKey}
        bind:value={
          () => profile.hotkey,
          (v) => updateProfile(index, { hotkey: v })
        }
      />
      {#if invalidHotkeys[index]}
        <p class="warning">
          {requiresBaseKey
            ? 'This hotkey cannot be registered on macOS; add a letter, number, function key, or Space.'
            : 'Choose a valid hotkey for this profile.'}
        </p>
      {/if}
      {#if conflictsByIndex.has(index)}
        <p class="warning">
          Conflicts with {conflictsByIndex.get(index)?.join(', ')} — one key press could fire
          either.
        </p>
      {/if}
    </Field>

    <Field label="Engine" for="profile-{index}-engine">
      <Select
        id="profile-{index}-engine"
        options={ENGINE_OPTIONS}
        bind:value={
          () => profile.engine.active,
          (v) => updateProfile(index, { engine: { active: v as EngineKind } })
        }
      />
    </Field>

    {#if profile.engine.active === 'whisper'}
      <Field label="Whisper model" for="profile-{index}-model">
        <Select
          id="profile-{index}-model"
          options={whisperOptions}
          bind:value={
            () => profile.engine.whisper_model,
            (v) => updateProfile(index, { engine: { whisper_model: v } })
          }
        />
      </Field>
    {:else if profile.engine.active === 'sherpa'}
      <Field label="Sherpa model" for="profile-{index}-model">
        <Select
          id="profile-{index}-model"
          options={sherpaOptions}
          bind:value={
            () => profile.engine.sherpa_model ?? '',
            (v) => updateProfile(index, { engine: { sherpa_model: v === '' ? null : v } })
          }
        />
      </Field>
    {:else}
      <Field label="Cloud base URL" for="profile-{index}-cloud-url">
        <TextInput
          id="profile-{index}-cloud-url"
          type="url"
          bind:value={
            () => profile.engine.cloud.base_url,
            (v) => updateProfile(index, { engine: { cloud: { base_url: v } } })
          }
        />
      </Field>
      <Field
        label="Cloud model"
        for="profile-{index}-cloud-model"
        hint="Choose a preset or enter any model supported by your OpenAI-compatible endpoint."
      >
        <TextInput
          id="profile-{index}-cloud-model"
          bind:value={
            () => profile.engine.cloud.model,
            (v) => updateProfile(index, { engine: { cloud: { model: v } } })
          }
        />
        <div class="model-presets" aria-label="Cloud model presets">
          {#each CLOUD_MODEL_PRESETS as preset (preset.value)}
            <button
              type="button"
              class:active={profile.engine.cloud.model === preset.value}
              aria-pressed={profile.engine.cloud.model === preset.value}
              onclick={() => updateProfile(index, { engine: { cloud: { model: preset.value } } })}
            >{preset.label}</button>
          {/each}
        </div>
      </Field>
    {/if}

    {#if profile.engine.active !== 'sherpa'}
      <Field
        label="Recognition prompt"
        for="profile-{index}-recognition-prompt-mode"
        hint={profile.engine.active === 'cloud'
          ? 'Sent to the transcription endpoint before recognition. Recommended sends dictionary terms only; Custom adds your guidance. This is separate from LLM refinement.'
          : 'Guides Whisper before recognition. Dictionary terms are added in every mode; this does not call the refinement LLM.'}
      >
        <Select
          id="profile-{index}-recognition-prompt-mode"
          options={RECOGNITION_PROMPT_OPTIONS}
          bind:value={
            () => profile.recognition.prompt_mode,
            (v) =>
              updateProfile(index, {
                recognition: { prompt_mode: v as RecognitionPromptMode },
              })
          }
        />
      </Field>
      {#if profile.recognition.prompt_mode === 'custom'}
        <Field
          label="Custom recognition prompt"
          for="profile-{index}-recognition-prompt"
          hint="Keep it concise: punctuation examples, language mix, and desired spellings work better than editing instructions."
        >
          <textarea
            id="profile-{index}-recognition-prompt"
            rows="4"
            bind:value={
              () => profile.recognition.custom_prompt,
              (v) => updateProfile(index, { recognition: { custom_prompt: v } })
            }
          ></textarea>
        </Field>
      {/if}
    {/if}

    <Field
      label="Live preview"
      for="profile-{index}-preview"
      hint="A streaming model that shows words in the HUD while you speak. The inserted text always comes from the engine above, never from this. Off by default. Download the model on the Engines page first — one selected before it is downloaded stays silent until settings are next saved or the app restarts."
    >
      <Select
        id="profile-{index}-preview"
        options={previewOptions}
        bind:value={
          () => profile.draft?.model ?? '',
          (v) => updateProfile(index, { draft: v === '' ? null : { model: v } })
        }
      />
    </Field>

    <Field
      label="Refine transcripts"
      for="profile-{index}-refine"
      hint="Also needs the master switch on the Refinement page."
    >
      <Toggle
        id="profile-{index}-refine"
        bind:checked={
          () => profile.refine.enabled,
          (v) => updateProfile(index, { refine: { enabled: v } })
        }
      />
    </Field>

    {#if profile.refine.enabled}
      <Field label="Tone" for="profile-{index}-tone">
        <Select
          id="profile-{index}-tone"
          options={TONE_OPTIONS}
          bind:value={
            () => profile.refine.tone,
            (v) => updateProfile(index, { refine: { tone: v as Tone } })
          }
        />
      </Field>
      <Field
        label="Refinement instructions"
        for="profile-{index}-refine-instructions"
        hint="Optional editing preferences for the LLM pass, for example punctuation or formatting. Built-in rules still preserve meaning and language."
      >
        <textarea
          id="profile-{index}-refine-instructions"
          rows="4"
          bind:value={
            () => profile.refine.instructions,
            (v) => updateProfile(index, { refine: { instructions: v } })
          }
        ></textarea>
      </Field>
    {/if}

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
  </Section>
{/each}

<Section title="Add a profile" description="Bind another hotkey to a language, engine, and refinement policy.">
  <div class="profile-actions">
    <button type="button" onclick={addProfile}>Add profile</button>
  </div>
</Section>

<style>
  .error {
    color: var(--danger);
    font-size: 13px;
  }

  .warning {
    color: var(--warning-text);
    background: var(--warning-bg);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-sm);
    font-size: 12px;
  }

  .profile-actions {
    display: flex;
  }

  .model-presets {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
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

  button {
    padding: 5px var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-elevated);
    cursor: pointer;
    font-size: 13px;
  }

  button:disabled {
    opacity: 0.55;
    cursor: not-allowed;
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

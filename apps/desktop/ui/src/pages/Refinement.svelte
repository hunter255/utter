<script lang="ts">
  import { onMount } from 'svelte'

  import Section from '../lib/components/Section.svelte'
  import Field from '../lib/components/Field.svelte'
  import Select from '../lib/components/Select.svelte'
  import TextInput from '../lib/components/TextInput.svelte'
  import Toggle from '../lib/components/Toggle.svelte'
  import * as api from '../lib/api'
  import { t } from '../lib/i18n'
  import { settingsStore } from '../lib/stores'

  let settings = $derived($settingsStore!)

  interface Preset {
    label: string
    base_url: string
    model: string
  }

  const PRESETS: Record<string, Preset> = {
    deepseek: { label: 'DeepSeek', base_url: 'https://api.deepseek.com', model: 'deepseek-v4-flash' },
    groq: { label: 'Groq', base_url: 'https://api.groq.com/openai/v1', model: 'llama-3.1-8b-instant' },
    ollama: { label: 'Ollama (local)', base_url: 'http://localhost:11434/v1', model: 'llama3.2' },
    openai: { label: 'OpenAI', base_url: 'https://api.openai.com/v1', model: 'gpt-4o-mini' },
    openrouter: {
      label: 'OpenRouter',
      base_url: 'https://openrouter.ai/api/v1',
      model: 'openai/gpt-4o-mini',
    },
  }

  let presetOptions = $derived([
    { value: '', label: $t('connections.choosePreset') },
    ...Object.entries(PRESETS).map(([value, p]) => ({
      value,
      label: value === 'ollama' ? $t('connections.ollamaLocal') : p.label,
    })),
  ])

  let selectedPreset = $state('')

  function applyPreset(key: string) {
    selectedPreset = key
    const preset = PRESETS[key]
    if (!preset) return
    settingsStore.patch({ refine: { base_url: preset.base_url, model: preset.model } })
  }

  let refineConfigured = $state(false)
  let refineApiKey = $state('')
  let refineKeyJustSaved = $state(false)
  let refineKeyError = $state('')
  let sttConfigured = $state(false)
  let sttApiKey = $state('')
  let sttKeyJustSaved = $state(false)
  let sttKeyError = $state('')

  onMount(async () => {
    const [refineKeyPresent, sttKeyPresent] = await Promise.all([
      api.hasApiKey('refine').catch(() => false),
      api.hasApiKey('stt').catch(() => false),
    ])
    refineConfigured = refineKeyPresent
    sttConfigured = sttKeyPresent
  })

  async function saveSttKey() {
    if (!sttApiKey.trim()) return
    sttKeyError = ''
    try {
      await api.setApiKey('stt', sttApiKey)
    } catch (err) {
      sttKeyError = String(err)
      return
    }
    sttApiKey = ''
    sttConfigured = true
    sttKeyJustSaved = true
    setTimeout(() => {
      sttKeyJustSaved = false
    }, 2000)
  }

  async function saveRefineKey() {
    if (!refineApiKey.trim()) return
    refineKeyError = ''
    try {
      await api.setApiKey('refine', refineApiKey)
    } catch (err) {
      refineKeyError = String(err)
      return
    }
    refineApiKey = ''
    refineConfigured = true
    refineKeyJustSaved = true
    setTimeout(() => {
      refineKeyJustSaved = false
    }, 2000)
  }

  // The sample text is a testing convenience, not a real app setting, so it
  // is not part of the Rust `Settings` schema. It's persisted to
  // localStorage instead (mirroring the ONBOARDED_KEY pattern in App.svelte)
  // so it survives both page navigation (which unmounts/remounts this
  // component) and an app restart.
  const SAMPLE_TEXT_KEY = 'utter.refinement.sampleText'
  let testSample = $state(
    localStorage.getItem(SAMPLE_TEXT_KEY) ?? $t('connections.defaultSample'),
  )
  let testResult = $state('')
  let testError = $state('')
  let testing = $state(false)

  $effect(() => {
    localStorage.setItem(SAMPLE_TEXT_KEY, testSample)
  })

  async function runTest() {
    testing = true
    testResult = ''
    testError = ''
    try {
      testResult = await api.testRefine(testSample)
    } catch (err) {
      testError = String(err)
    } finally {
      testing = false
    }
  }
</script>

<header class="page-heading">
  <h1>{$t('connections.title')}</h1>
  <p>{$t('connections.description')}</p>
</header>

<Section
  title={$t('connections.cloud.title')}
  description={$t('connections.cloud.description')}
>
  <Field label={$t('connections.apiKey')} for="cloud-stt-key">
    <div class="key-row">
      <TextInput
        id="cloud-stt-key"
        type="password"
        placeholder="sk-…"
        bind:value={() => sttApiKey, (value) => (sttApiKey = value)}
      />
      <button type="button" onclick={saveSttKey} disabled={!sttApiKey.trim()}>{$t('common.save')}</button>
      {#if sttKeyJustSaved}
        <span class="badge badge-installed">{$t('common.saved')}</span>
      {:else if sttConfigured}
        <span class="badge badge-installed">{$t('common.configured')}</span>
      {:else}
        <span class="badge badge-missing">{$t('common.notSet')}</span>
      {/if}
    </div>
    {#if sttKeyError}
      <p class="error">{$t('connections.keySaveFailed', { error: sttKeyError })}</p>
    {/if}
  </Field>
</Section>

<Section
  title={$t('connections.refinement.title')}
  description={$t('connections.refinement.description')}
>
  <Field
    label={$t('connections.pause')}
    for="refine-paused"
    hint={$t('connections.pauseHint')}
  >
    <Toggle
      id="refine-paused"
      bind:checked={
        () => !settings.refine.enabled,
        (paused) => settingsStore.patch({ refine: { enabled: !paused } })
      }
    />
  </Field>

  <Field
    label={$t('connections.providerPreset')}
    for="preset"
    hint={$t('connections.providerPresetHint')}
  >
    <Select id="preset" options={presetOptions} bind:value={() => selectedPreset, applyPreset} />
  </Field>

  <Field label={$t('connections.baseUrl')} for="refine-url">
    <TextInput
      id="refine-url"
      type="url"
      bind:value={
        () => settings.refine.base_url,
        (v) => settingsStore.patch({ refine: { base_url: v } })
      }
    />
  </Field>

  <Field label={$t('connections.model')} for="refine-model">
    <TextInput
      id="refine-model"
      bind:value={
        () => settings.refine.model,
        (v) => settingsStore.patch({ refine: { model: v } })
      }
    />
  </Field>

  <Field
    label={$t('connections.timeout')}
    for="refine-timeout"
    hint={$t('connections.timeoutHint')}
  >
    <TextInput
      id="refine-timeout"
      type="number"
      min={1}
      max={120}
      bind:value={
        () => String(settings.refine.timeout_secs),
        (v) => settingsStore.patch({ refine: { timeout_secs: Math.max(1, Math.round(Number(v) || 10)) } })
      }
    />
  </Field>

  <Field label={$t('connections.apiKey')} for="refine-key">
    <div class="key-row">
      <TextInput
        id="refine-key"
        type="password"
        placeholder="sk-…"
        bind:value={() => refineApiKey, (v) => (refineApiKey = v)}
      />
      <button type="button" onclick={saveRefineKey} disabled={!refineApiKey.trim()}>{$t('common.save')}</button>
      {#if refineKeyJustSaved}
        <span class="badge badge-installed">{$t('common.saved')}</span>
      {:else if refineConfigured}
        <span class="badge badge-installed">{$t('common.configured')}</span>
      {:else}
        <span class="badge badge-missing">{$t('common.notSet')}</span>
      {/if}
    </div>
    {#if refineKeyError}
      <p class="error">{$t('connections.keySaveFailed', { error: refineKeyError })}</p>
    {/if}
  </Field>
</Section>

<Section
  title={$t('connections.test.title')}
  description={$t('connections.test.description')}
>
  <Field label={$t('connections.sampleText')} for="test-sample">
    <textarea id="test-sample" bind:value={testSample} rows="3"></textarea>
  </Field>
  <div class="test-actions">
    <button type="button" onclick={runTest} disabled={testing || !testSample.trim()}>
      {testing ? $t('common.testing') : $t('common.test')}
    </button>
  </div>
  {#if testResult}
    <div class="result">{testResult}</div>
  {/if}
  {#if testError}
    <div class="result error">{testError}</div>
  {/if}
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

  .page-heading p {
    color: var(--text-muted);
    font-size: 13px;
  }

  .key-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
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

  .badge {
    font-size: 11px;
    font-weight: 600;
    padding: 2px var(--space-2);
    border-radius: 999px;
  }

  .badge-installed {
    background: var(--success);
    color: var(--accent-contrast);
  }

  .badge-missing {
    background: var(--bg-sunken);
    color: var(--text-muted);
  }

  .error {
    color: var(--danger);
    font-size: 13px;
  }

  textarea {
    width: 100%;
    max-width: 480px;
    padding: var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--text);
    font-size: 13px;
    resize: vertical;
  }

  .test-actions {
    display: flex;
  }

  .result {
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-sm);
    background: var(--bg-sunken);
    font-size: 13px;
    white-space: pre-wrap;
  }

  .result.error {
    background: var(--danger-bg);
    color: var(--danger);
  }
</style>

<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import { getCurrentWindow } from '@tauri-apps/api/window'

  import HotkeyPicker from '../lib/components/HotkeyPicker.svelte'
  import MacosPermissionRecovery from '../lib/components/MacosPermissionRecovery.svelte'
  import Select from '../lib/components/Select.svelte'
  import * as api from '../lib/api'
  import { hasBaseKey, parseChordTokens } from '../lib/hotkey'
  import {
    modelCapabilityLabel,
    modelLanguageWarning,
    transcriptionLanguageOptions,
    transcriptionModelOptions,
    transcriptionModels,
  } from '../lib/models'
  import { settingsStore } from '../lib/stores'
  import type {
    EngineCfg,
    ModelInfo,
    PermissionKind,
    PermissionReport,
    PermissionStatus,
    PlatformCapabilities,
  } from '../lib/types'

  interface Props {
    onDone: () => void
    capabilities: PlatformCapabilities
  }

  let { onDone, capabilities }: Props = $props()

  let settings = $derived($settingsStore!)
  let requiresBaseKey = $derived(!capabilities.modifier_only_hotkeys)
  let hotkeyValid = $derived.by(() => {
    const hotkey = settings.profiles[0]?.hotkey ?? ''
    return parseChordTokens(hotkey) !== null && (!requiresBaseKey || hasBaseKey(hotkey))
  })

  const STEPS = ['Welcome', 'Microphone', 'Model', 'Hotkey', 'Permissions', 'Done'] as const
  let step = $state(0)

  function next() {
    if (step === 3 && !hotkeyValid) return
    if (step < STEPS.length - 1) step += 1
  }
  function back() {
    if (step > 0) step -= 1
  }

  // --- Step: microphone ---
  let devices = $state<string[]>([])
  let devicesError = $state('')
  let devicesChecked = $state(false)

  async function checkMic() {
    try {
      devices = await api.listDevices()
    } catch (err) {
      devicesError = String(err)
    } finally {
      devicesChecked = true
    }
  }

  // --- Step: model ---
  let models = $state<ModelInfo[]>([])
  let modelsError = $state('')
  let progress = $state<Record<string, { done: number; total: number }>>({})
  let activeDownloadId = $state<string | null>(null)
  let cancellingDownload = $state(false)
  let unlistenProgress: (() => void) | undefined

  // A fresh profile starts on sherpa, but onboarding is where users should be able to choose
  // either final-transcript engine. Streaming preview models stay out of this list because they
  // cannot replace the model whose text is inserted into the focused app.
  let profileEngine = $derived(settings.profiles[0]?.engine.active ?? 'whisper')
  let profileLanguage = $derived(settings.profiles[0]?.language ?? '')
  let profileModelId = $derived(
    profileEngine === 'sherpa'
      ? settings.profiles[0]?.engine.sherpa_model
      : profileEngine === 'whisper'
        ? settings.profiles[0]?.engine.whisper_model
        : null,
  )
  let localModels = $derived(transcriptionModels(models))
  let languagePickerOptions = $derived(transcriptionLanguageOptions(models))
  let modelPickerOptions = $derived([
    {
      value: '',
      label: models.length === 0 ? 'Loading models…' : 'Choose a local model',
    },
    ...transcriptionModelOptions(models),
  ])
  let selectedModel = $derived(
    localModels.find((model) => model.id === profileModelId) ?? null,
  )
  let selectedModelInstalled = $derived(selectedModel?.installed ?? false)
  let languageWarning = $derived(modelLanguageWarning(selectedModel, profileLanguage))

  function selectLanguage(language: string) {
    settingsStore.patch({
      profiles: settings.profiles.map((profile, index) =>
        index === 0 ? { ...profile, language } : profile,
      ),
    })
  }

  function selectModel(id: string) {
    if (!id) return

    const model = localModels.find((candidate) => candidate.id === id)
    const profile = settings.profiles[0]
    if (!model || !profile) return

    const engine: EngineCfg =
      model.engine === 'whisper'
        ? { ...profile.engine, active: 'whisper', whisper_model: model.id }
        : { ...profile.engine, active: 'sherpa', sherpa_model: model.id }

    settingsStore.patch({
      profiles: settings.profiles.map((candidate, index) =>
        index === 0 ? { ...candidate, engine } : candidate,
      ),
    })
  }

  async function refreshModels() {
    try {
      models = await api.listModels()
      modelsError = ''
    } catch (err) {
      modelsError = String(err)
    }
  }

  function progressPercent(id: string): number | null {
    const p = progress[id]
    if (!p || p.total <= 0) return null
    return Math.min(100, Math.round((p.done / p.total) * 100))
  }

  async function install(id: string) {
    if (activeDownloadId) return
    activeDownloadId = id
    modelsError = ''
    try {
      const outcome = await api.downloadModel(id)
      if (outcome === 'installed') await refreshModels()
    } catch (err) {
      modelsError = `Failed to download "${id}": ${String(err)}`
    } finally {
      activeDownloadId = null
      cancellingDownload = false
      const rest = { ...progress }
      delete rest[id]
      progress = rest
    }
  }

  async function cancelDownload(id: string) {
    if (activeDownloadId !== id || cancellingDownload) return
    cancellingDownload = true
    modelsError = ''
    try {
      await api.cancelModelDownload(id)
    } catch (err) {
      cancellingDownload = false
      modelsError = `Failed to cancel "${id}": ${String(err)}`
    }
  }

  // --- Step: permissions ---
  let permissions = $state<PermissionReport | null>(null)
  let permissionsError = $state('')
  let fixCopied = $state(false)
  let permissionBusy = $state<PermissionKind | null>(null)
  let unlistenFocus: (() => void) | undefined

  async function checkPermissions() {
    try {
      permissions = await api.permissionsReport()
      permissionsError = ''
    } catch (err) {
      permissionsError = String(err)
    }
  }

  async function requestPermission(kind: PermissionKind) {
    permissionBusy = kind
    permissionsError = ''
    try {
      permissions = await api.requestPermission(kind)
      if (kind === 'microphone' && permissions.platform === 'macos' && permissions.microphone === 'granted') {
        devicesChecked = false
        await checkMic()
      }
    } catch (err) {
      permissionsError = String(err)
    } finally {
      permissionBusy = null
    }
  }

  function permissionMark(status: PermissionStatus): string {
    if (status === 'granted') return '✓'
    if (status === 'denied') return '✗'
    return '•'
  }

  async function copyFixCommand() {
    if (!permissions || permissions.platform !== 'linux') return
    try {
      await navigator.clipboard.writeText(permissions.fix_command)
      fixCopied = true
      setTimeout(() => {
        fixCopied = false
      }, 1500)
    } catch {
      // Best-effort; clipboard access may be denied.
    }
  }

  onMount(() => {
    void refreshModels()
    api.onModelProgress((p) => {
      progress = { ...progress, [p.id]: { done: p.done, total: p.total } }
    }).then((fn) => {
      unlistenProgress = fn
    })
    getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused && permissions?.platform === 'macos') void checkPermissions()
    }).then((fn) => {
      unlistenFocus = fn
    })
  })

  onDestroy(() => {
    unlistenProgress?.()
    unlistenFocus?.()
  })

  $effect(() => {
    if (step === 1 && !permissions && !permissionsError) void checkPermissions()
    if (
      step === 1 &&
      !devicesChecked &&
      (permissions?.platform !== 'macos' || permissions.microphone === 'granted')
    ) void checkMic()
    if (step === 4 && !permissions && !permissionsError) void checkPermissions()
  })
</script>

<div class="onboarding">
  <div class="card">
    <div class="steps">
      {#each STEPS as label, i (label)}
        <span class="step-dot" class:active={i === step} class:done={i < step}>{i + 1}</span>
      {/each}
    </div>

    {#if step === 0}
      <h1>Welcome to Utter</h1>
      <p>A quick, skippable setup: microphone, a speech model, your hotkey, and permissions.</p>
    {:else if step === 1}
      <h1>Microphone</h1>
      {#if permissions?.platform === 'macos' && permissions.microphone !== 'granted'}
        <p class="muted">Utter needs microphone access to record speech for transcription.</p>
        {#if permissions.microphone === 'not_determined'}
          <button
            type="button"
            onclick={() => requestPermission('microphone')}
            disabled={permissionBusy !== null}
          >{permissionBusy === 'microphone' ? 'Requesting…' : 'Allow microphone'}</button>
        {:else if permissions.microphone === 'denied'}
          <p class="warn">
            Microphone access is off. Enable Utter in System Settings → Privacy & Security →
            Microphone, then return here.
          </p>
          <MacosPermissionRecovery
            kind="microphone"
            command={permissions.microphone_reset_command}
            onError={(message) => (permissionsError = message)}
          />
          <p class="muted">
            If Utter is missing or the status is stale, copy the command, quit Utter, run it in
            Terminal, then reopen the app and allow access again.
          </p>
        {:else}
          <p class="warn">Microphone permission is unavailable on this Mac.</p>
        {/if}
      {:else}
        <p class="muted">
          This confirms your system reports an input device; live recording is tested when you
          dictate.
        </p>
        {#if devicesError}
          <p class="error">{devicesError}</p>
        {:else if devicesChecked}
          {#if devices.length === 0}
            <p class="warn">No input devices were found. Check your microphone connection.</p>
          {:else}
            <p>Found {devices.length} input device{devices.length === 1 ? '' : 's'}:</p>
            <ul>
              {#each devices as device (device)}
                <li>{device}</li>
              {/each}
            </ul>
          {/if}
        {:else}
          <p class="muted">Checking…</p>
        {/if}
      {/if}
    {:else if step === 2}
      <h1>Speech model</h1>
      <p class="muted">
        Choose the language and local model for your first profile. All final-transcript models
        in the catalog are available here; live-preview-only models stay separate.
      </p>
      <div class="picker-field">
        <label for="onboarding-language">Language</label>
        <Select
          id="onboarding-language"
          options={languagePickerOptions}
          bind:value={() => profileLanguage, selectLanguage}
        />
      </div>
      <div class="picker-field">
        <label for="onboarding-model">Model</label>
        <Select
          id="onboarding-model"
          options={modelPickerOptions}
          disabled={models.length === 0 || activeDownloadId !== null}
          bind:value={
            () => (profileEngine === 'cloud' ? '' : (profileModelId ?? '')),
            selectModel
          }
        />
      </div>
      {#if modelsError}
        <p class="error">{modelsError}</p>
      {/if}
      {#if languageWarning}
        <p class="warn">{languageWarning}</p>
      {/if}
      {#if profileEngine === 'cloud' && !selectedModel}
        <p class="muted">
          Your default profile dictates through a cloud speech-to-text endpoint. Configure its
          API key under Settings &gt; Engines after finishing setup, or choose a local model above.
        </p>
      {:else if selectedModel}
        <ul class="model-list">
          <li>
            <div class="model-row">
              <div class="model-info">
                <span class="model-label">{selectedModel.label}</span>
                <span class="model-size">
                  {selectedModel.engine === 'whisper' ? 'Whisper' : 'Sherpa-onnx'} ·
                  {modelCapabilityLabel(selectedModel)} · {selectedModel.size_mb} MB
                </span>
              </div>
              {#if selectedModel.installed}
                <span class="badge">Installed</span>
              {:else if activeDownloadId === selectedModel.id}
                <button
                  type="button"
                  class="cancel"
                  onclick={() => cancelDownload(selectedModel.id)}
                  disabled={cancellingDownload}
                >{cancellingDownload ? 'Cancelling…' : 'Cancel'}</button>
              {:else}
                <button
                  type="button"
                  onclick={() => install(selectedModel.id)}
                  disabled={activeDownloadId !== null}
                >Install</button>
              {/if}
            </div>
            {#if activeDownloadId === selectedModel.id}
              <div class="progress-track">
                <div
                  class="progress-fill"
                  style:width="{progressPercent(selectedModel.id) ?? 0}%"
                ></div>
              </div>
            {/if}
          </li>
        </ul>
        {#if selectedModelInstalled}
          <p class="ok">This model is installed — you're ready to dictate.</p>
        {/if}
      {:else if models.length > 0}
        <p class="warn">Choose a model before continuing if you want to dictate locally.</p>
      {/if}
    {:else if step === 3}
      <h1>Hotkey</h1>
      <p class="muted">Pick the key combination that starts/stops dictation.</p>
      <HotkeyPicker
        requireBaseKey={requiresBaseKey}
        bind:value={
          () => settings.profiles[0].hotkey,
          (v) =>
            settingsStore.patch({
              profiles: settings.profiles.map((p, i) => (i === 0 ? { ...p, hotkey: v } : p)),
            })
        }
      />
      {#if !hotkeyValid}
        <p class="warn">
          {requiresBaseKey
            ? 'macOS needs a base key; modifiers are optional. Try `, Insert, F5, or ctrl+alt+space.'
            : 'Choose a hotkey before continuing.'}
        </p>
      {/if}
    {:else if step === 4}
      <h1>Permissions</h1>
      {#if permissionsError}
        <p class="error">{permissionsError}</p>
      {:else if permissions}
        {#if permissions.platform === 'linux'}
          <p class="muted">Linux hotkeys and text injection need two OS-level permissions.</p>
          <ul class="perm-list">
            <li>
              <span class="perm-status" class:ok={permissions.input_group}>
                {permissions.input_group ? '✓' : '✗'}
              </span>
              Input device group membership
            </li>
            <li>
              <span class="perm-status" class:ok={permissions.uinput_writable}>
                {permissions.uinput_writable ? '✓' : '✗'}
              </span>
              /dev/uinput writable
            </li>
          </ul>
          {#if !permissions.input_group || !permissions.uinput_writable}
            <pre class="fix-command">{permissions.fix_command}</pre>
            <button type="button" onclick={copyFixCommand}>{fixCopied ? 'Copied' : 'Copy fix command'}</button>
          {:else}
            <p class="ok">All required permissions are already granted.</p>
          {/if}
        {:else if permissions.platform === 'macos'}
          <p class="muted">
            These permissions are requested only when you press an Allow button. You can
            continue with reduced functionality if either remains off.
          </p>
          <ul class="perm-list">
            <li>
              <span class="perm-status" class:ok={permissions.microphone === 'granted'}>
                {permissionMark(permissions.microphone)}
              </span>
              Microphone — {permissions.microphone.replace('_', ' ')}
              {#if permissions.microphone === 'not_determined'}
                <button
                  type="button"
                  onclick={() => requestPermission('microphone')}
                  disabled={permissionBusy !== null}
                >{permissionBusy === 'microphone' ? 'Requesting…' : 'Allow'}</button>
              {/if}
            </li>
            <li>
              <span class="perm-status" class:ok={permissions.text_injection === 'granted'}>
                {permissionMark(permissions.text_injection)}
              </span>
              Paste and caret-relative HUD — {permissions.text_injection.replace('_', ' ')}
              {#if permissions.text_injection === 'not_determined'}
                <button
                  type="button"
                  onclick={() => requestPermission('text_injection')}
                  disabled={permissionBusy !== null}
                >{permissionBusy === 'text_injection' ? 'Requesting…' : 'Allow'}</button>
              {/if}
            </li>
          </ul>
          {#if permissions.microphone === 'denied' || permissions.text_injection === 'denied'}
            <p class="warn">
              Enable the denied access in System Settings → Privacy & Security, then return to
              Utter and check again. Dictation needs Microphone; automatic paste and precise
              HUD position need Accessibility.
            </p>
            {#if permissions.microphone === 'denied'}
              <strong class="recovery-label">Microphone recovery</strong>
              <MacosPermissionRecovery
                kind="microphone"
                command={permissions.microphone_reset_command}
                onError={(message) => (permissionsError = message)}
              />
            {/if}
            {#if permissions.text_injection === 'denied'}
              <strong class="recovery-label">Accessibility recovery</strong>
              <MacosPermissionRecovery
                kind="text_injection"
                command={permissions.text_injection_reset_command}
                onError={(message) => (permissionsError = message)}
              />
            {/if}
            <p class="muted">
              Use reset only if the System Settings entry is missing or stale: copy the command,
              quit Utter, run it in Terminal, then reopen Utter and allow access again.
            </p>
            <button type="button" onclick={checkPermissions}>Check again</button>
          {:else if permissions.microphone === 'granted' && permissions.text_injection === 'granted'}
            <p class="ok">All required permissions are granted.</p>
          {/if}
        {:else}
          <p class="muted">
            Permission setup for {permissions.os} is not available in this build yet. You can
            continue and configure platform access later.
          </p>
        {/if}
      {:else}
        <p class="muted">Checking…</p>
      {/if}
    {:else if step === 5}
      <h1>You're all set</h1>
      <p>You can revisit any of this later from the settings sidebar.</p>
    {/if}

    <div class="actions">
      {#if step > 0}
        <button type="button" onclick={back} disabled={activeDownloadId !== null}>Back</button>
      {/if}
      <div class="spacer"></div>
      {#if step < STEPS.length - 1}
        <button type="button" class="ghost" onclick={onDone} disabled={activeDownloadId !== null}>Skip setup</button>
        <button
          type="button"
          class="primary"
          onclick={next}
          disabled={activeDownloadId !== null || (step === 3 && !hotkeyValid)}
        >
          Continue
        </button>
      {:else}
        <button type="button" class="primary" onclick={onDone}>Finish</button>
      {/if}
    </div>
  </div>
</div>

<style>
  .onboarding {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100vh;
    background: var(--bg);
  }

  .card {
    width: 100%;
    max-width: 480px;
    padding: var(--space-6);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-elevated);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  .steps {
    display: flex;
    gap: var(--space-2);
    margin-bottom: var(--space-2);
  }

  .step-dot {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 11px;
    background: var(--bg-sunken);
    color: var(--text-muted);
  }

  .step-dot.active {
    background: var(--accent);
    color: var(--accent-contrast);
  }

  .step-dot.done {
    background: var(--success);
    color: var(--accent-contrast);
  }

  h1 {
    font-size: 18px;
    font-weight: 700;
  }

  .muted {
    color: var(--text-muted);
    font-size: 13px;
  }

  .error {
    color: var(--danger);
    font-size: 13px;
  }

  .warn {
    color: var(--warning-text);
    background: var(--warning-bg);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-sm);
    font-size: 13px;
  }

  .ok {
    color: var(--success);
    font-size: 13px;
  }

  .picker-field {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .picker-field label {
    font-size: 13px;
    font-weight: 600;
  }

  ul {
    margin: 0;
    padding-left: var(--space-4);
    font-size: 13px;
  }

  .model-list {
    list-style: none;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .model-list li {
    padding: var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }

  .model-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .model-info {
    display: flex;
    flex-direction: column;
  }

  .model-label {
    font-size: 13px;
    font-weight: 500;
  }

  .model-size {
    font-size: 12px;
    color: var(--text-muted);
  }

  .badge {
    font-size: 11px;
    font-weight: 600;
    padding: 2px var(--space-2);
    border-radius: 999px;
    background: var(--success);
    color: var(--accent-contrast);
  }

  .progress-track {
    margin-top: var(--space-2);
    height: 6px;
    border-radius: 999px;
    background: var(--bg-sunken);
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    transition: width 150ms ease;
  }

  .perm-list {
    list-style: none;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .perm-status {
    display: inline-block;
    width: 1.5em;
    color: var(--danger);
    font-weight: 700;
  }

  .perm-status.ok {
    color: var(--success);
  }

  .fix-command {
    background: var(--bg-sunken);
    padding: var(--space-2);
    border-radius: var(--radius-sm);
    font-size: 12px;
    overflow-x: auto;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .recovery-label {
    font-size: 13px;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-top: var(--space-3);
  }

  .spacer {
    flex: 1;
  }

  button {
    padding: 6px var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    cursor: pointer;
    font-size: 13px;
  }

  button:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  button.cancel {
    color: var(--danger);
  }

  button.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-contrast);
    font-weight: 600;
  }

  button.primary:not(:disabled):hover {
    background: var(--accent-hover);
    border-color: var(--accent-hover);
  }

  button.ghost {
    background: none;
    border-color: transparent;
    color: var(--text-muted);
  }

  button.ghost:not(:disabled):hover {
    background: var(--surface-hover);
  }
</style>

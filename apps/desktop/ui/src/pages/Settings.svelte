<script lang="ts">
  import { onMount } from 'svelte'

  import * as api from '../lib/api'
  import Field from '../lib/components/Field.svelte'
  import MacosPermissionRecovery from '../lib/components/MacosPermissionRecovery.svelte'
  import Section from '../lib/components/Section.svelte'
  import Select from '../lib/components/Select.svelte'
  import Slider from '../lib/components/Slider.svelte'
  import TextInput from '../lib/components/TextInput.svelte'
  import Toggle from '../lib/components/Toggle.svelte'
  import { settingsStore } from '../lib/stores'
  import type {
    DictationMode,
    DictationPhase,
    HudPlacement,
    InjectionPreference,
    PermissionKind,
    PermissionStatus,
    PlatformCapabilities,
    Theme,
    UpdateCheck,
    UpdateProgressPayload,
  } from '../lib/types'
  import { inputSignal, rmsToMeter, smoothMeter } from '../hud/state'

  interface Props {
    capabilities: PlatformCapabilities
  }

  let { capabilities }: Props = $props()
  let settings = $derived($settingsStore!)

  const THEME_OPTIONS: { value: Theme; label: string }[] = [
    { value: 'system', label: 'Match system' },
    { value: 'light', label: 'Light' },
    { value: 'dark', label: 'Dark' },
  ]

  const MODE_OPTIONS: { value: DictationMode; label: string }[] = [
    { value: 'push_to_talk', label: 'Push to talk (hold hotkey)' },
    { value: 'toggle', label: 'Toggle (press to start, press to stop)' },
  ]

  const INJECTION_OPTIONS: { value: InjectionPreference; label: string }[] = [
    { value: 'auto', label: 'Auto (best available)' },
    { value: 'clipboard_paste', label: 'Clipboard + paste' },
    { value: 'type', label: 'Simulated typing' },
    { value: 'clipboard_only', label: 'Clipboard only (no auto-paste)' },
  ]

  const HUD_PLACEMENT_OPTIONS: { value: HudPlacement; label: string }[] = [
    { value: 'auto', label: 'Automatic (near the caret)' },
    { value: 'pointer', label: 'Near the pointer' },
    { value: 'bottom_center', label: 'Bottom center' },
  ]

  const LOG_LEVEL_OPTIONS = ['trace', 'debug', 'info', 'warn', 'error'].map((value) => ({
    value,
    label: value,
  }))

  const MODEL_IDLE_OPTIONS = [
    { value: '900', label: 'After 15 minutes' },
    { value: '1800', label: 'After 30 minutes' },
    { value: '3600', label: 'After 1 hour' },
    { value: '0', label: 'Never' },
  ]

  let availableInjectionOptions = $derived(
    INJECTION_OPTIONS.filter((option) => capabilities.injection_methods.includes(option.value)),
  )
  let configuredInjectionAvailable = $derived(
    capabilities.injection_methods.includes(settings.advanced.injection),
  )
  let timeoutEnabled = $derived(settings.dictation.silence_timeout_secs !== null)
  let timeoutValue = $derived(String(settings.dictation.silence_timeout_secs ?? 30))

  let devices = $state<string[]>([])
  let devicesError = $state('')
  let microphonePermission = $state<PermissionStatus | null>(null)
  let microphoneResetCommand = $state('')
  let textInjectionPermission = $state<PermissionStatus | null>(null)
  let textInjectionResetCommand = $state('')
  let permissionBusy = $state<PermissionKind | null>(null)
  let permissionError = $state('')
  let inputMeter = $state(0)
  let dictationPhase = $state<DictationPhase>('idle')

  let diagnosticsBusy = $state<'open' | 'copy' | null>(null)
  let diagnosticsMessage = $state('')
  let diagnosticsError = $state('')
  let updaterBusy = $state<'check' | 'install' | null>(null)
  let updateCheck = $state<UpdateCheck | null>(null)
  let updateProgress = $state<UpdateProgressPayload | null>(null)
  let updateMessage = $state('')
  let updateError = $state('')
  let updatePercent = $derived.by(() => {
    const progress = updateProgress
    if (progress?.event !== 'progress' || !progress.total) return null
    return Math.min(100, Math.round((progress.downloaded / progress.total) * 100))
  })
  let deviceOptions = $derived([
    { value: '', label: 'System default' },
    ...devices.map((device) => ({ value: device, label: device })),
  ])

  function setTimeoutEnabled(enabled: boolean) {
    settingsStore.patch({
      dictation: { silence_timeout_secs: enabled ? Number(timeoutValue) || 30 : null },
    })
  }

  function setTimeoutValue(raw: string) {
    const seconds = Math.max(1, Math.round(Number(raw) || 0))
    settingsStore.patch({ dictation: { silence_timeout_secs: seconds } })
  }

  async function refreshMacosPermissions() {
    if (capabilities.os !== 'macos') return
    try {
      const report = await api.permissionsReport()
      if (report.platform === 'macos') {
        microphonePermission = report.microphone
        microphoneResetCommand = report.microphone_reset_command
        textInjectionPermission = report.text_injection
        textInjectionResetCommand = report.text_injection_reset_command
      }
      permissionError = ''
    } catch (err) {
      permissionError = String(err)
    }
  }

  async function requestMacosPermission(kind: PermissionKind) {
    permissionBusy = kind
    permissionError = ''
    try {
      const report = await api.requestPermission(kind)
      if (report.platform === 'macos') {
        microphonePermission = report.microphone
        microphoneResetCommand = report.microphone_reset_command
        textInjectionPermission = report.text_injection
        textInjectionResetCommand = report.text_injection_reset_command
      }
    } catch (err) {
      permissionError = String(err)
    } finally {
      permissionBusy = null
    }
  }

  async function openLogs() {
    diagnosticsBusy = 'open'
    diagnosticsMessage = ''
    diagnosticsError = ''
    try {
      await api.openLogs()
      diagnosticsMessage = 'Opened the log folder.'
    } catch (err) {
      diagnosticsError = String(err)
    } finally {
      diagnosticsBusy = null
    }
  }

  async function copyDiagnostics() {
    diagnosticsBusy = 'copy'
    diagnosticsMessage = ''
    diagnosticsError = ''
    try {
      const report = await api.copyDiagnostics()
      await navigator.clipboard.writeText(report)
      diagnosticsMessage = 'Safe diagnostic report copied.'
    } catch (err) {
      diagnosticsError = String(err)
    } finally {
      diagnosticsBusy = null
    }
  }

  async function checkForUpdate() {
    updaterBusy = 'check'
    updateError = ''
    updateMessage = ''
    updateProgress = null
    try {
      updateCheck = await api.checkForUpdate()
      updateMessage = updateCheck.update
        ? `Utter ${updateCheck.update.version} is ready to install.`
        : `Utter ${updateCheck.current_version} is up to date.`
    } catch (err) {
      updateError = String(err)
    } finally {
      updaterBusy = null
    }
  }

  async function installUpdate() {
    if (!updateCheck?.update) return
    updaterBusy = 'install'
    updateError = ''
    updateMessage = 'Downloading the signed update…'
    updateProgress = null
    try {
      await api.installUpdate()
      updateMessage = 'Update installed. Restarting…'
    } catch (err) {
      updateError = String(err)
    } finally {
      updaterBusy = null
    }
  }

  onMount(() => {
    let disposed = false
    let unlistenUpdate: (() => void) | undefined
    let unlistenDictation: (() => void) | undefined

    if (capabilities.updater) {
      void api
        .onUpdateProgress((progress) => (updateProgress = progress))
        .then((unlisten) => {
          if (disposed) unlisten()
          else unlistenUpdate = unlisten
        })
        .catch((err) => {
          if (!disposed) updateError = String(err)
        })
    }

    void api
      .onDictationState((payload) => {
        dictationPhase = payload.state
        inputMeter =
          payload.state === 'recording'
            ? smoothMeter(inputMeter, rmsToMeter(payload.level))
            : 0
      })
      .then((unlisten) => {
        if (disposed) unlisten()
        else unlistenDictation = unlisten
      })
      .catch(() => {})

    void refreshMacosPermissions()
    void api
      .listDevices()
      .then((available) => (devices = available))
      .catch((err) => (devicesError = String(err)))

    return () => {
      disposed = true
      unlistenUpdate?.()
      unlistenDictation?.()
    }
  })
</script>

<header class="page-heading">
  <h1>Settings</h1>
  <p>Recording, microphone, output, appearance, performance, and maintenance.</p>
</header>

<Section title="Appearance" description="How Utter looks and starts.">
  <Field label="Theme" for="theme">
    <Select
      id="theme"
      options={THEME_OPTIONS}
      bind:value={
        () => settings.general.theme,
        (value) => settingsStore.patch({ general: { theme: value as Theme } })
      }
    />
  </Field>

  <Field label="Launch at login" for="autostart">
    <Toggle
      id="autostart"
      bind:checked={
        () => settings.general.autostart,
        (value) => settingsStore.patch({ general: { autostart: value } })
      }
    />
  </Field>
</Section>

<Section
  title="Recording & microphone"
  description="How recording starts, when it stops, and which audio input it listens to."
>
  <Field label="Recording mode" for="mode">
    <Select
      id="mode"
      options={MODE_OPTIONS}
      bind:value={
        () => settings.dictation.mode,
        (value) => settingsStore.patch({ dictation: { mode: value as DictationMode } })
      }
    />
  </Field>

  <Field label="Silence timeout" hint="Automatically stop after continuous silence.">
    <div class="inline-row">
      <Toggle
        id="silence-timeout-enabled"
        bind:checked={() => timeoutEnabled, setTimeoutEnabled}
      />
      <span class="muted">{timeoutEnabled ? 'On' : 'Off'}</span>
      {#if timeoutEnabled}
        <TextInput
          type="number"
          min={1}
          max={600}
          bind:value={() => timeoutValue, setTimeoutValue}
        />
        <span class="muted">seconds</span>
      {/if}
    </div>
  </Field>

  <Field label="Audio input" for="audio-device">
    {#if devicesError}
      <p class="error">{devicesError}</p>
    {/if}
    <Select
      id="audio-device"
      options={deviceOptions}
      bind:value={
        () => settings.advanced.audio_device ?? '',
        (value) =>
          settingsStore.patch({ advanced: { audio_device: value === '' ? null : value } })
      }
    />
  </Field>

  {#if capabilities.os === 'macos'}
    <Field
      label="Microphone access"
      hint="macOS remembers this permission for Utter's stable app identity."
    >
      {#if permissionError}
        <p class="error">{permissionError}</p>
      {:else if microphonePermission === null}
        <p class="muted">Checking…</p>
      {:else}
        <p class:ok={microphonePermission === 'granted'}>
          Status: {microphonePermission.replace('_', ' ')}
        </p>
        {#if microphonePermission === 'not_determined'}
          <button
            type="button"
            onclick={() => requestMacosPermission('microphone')}
            disabled={permissionBusy !== null}
          >
            {permissionBusy === 'microphone' ? 'Requesting…' : 'Allow microphone'}
          </button>
        {:else if microphonePermission === 'denied'}
          <p class="warning">
            Enable Utter in System Settings → Privacy & Security → Microphone.
          </p>
          {#if microphoneResetCommand}
            <MacosPermissionRecovery
              kind="microphone"
              command={microphoneResetCommand}
              onError={(message) => (permissionError = message)}
            />
          {/if}
          <button type="button" onclick={refreshMacosPermissions}>Check again</button>
        {/if}
      {/if}
    </Field>
  {/if}

  <Field
    label="Live input level"
    hint="Start dictation with a profile hotkey to verify that Utter hears your microphone."
  >
    <div class="meter-row">
      <div
        class="input-meter"
        role="meter"
        aria-label="Microphone input level"
        aria-valuemin="0"
        aria-valuemax="100"
        aria-valuenow={Math.round(inputMeter * 100)}
      >
        <span style:width="{Math.round(inputMeter * 100)}%"></span>
      </div>
      <span class="muted">
        {dictationPhase === 'recording'
          ? inputSignal(inputMeter) === 'voice'
            ? 'Voice detected'
            : inputSignal(inputMeter) === 'quiet'
              ? 'Quiet input'
              : 'No signal'
          : 'Waiting for recording'}
      </span>
    </div>
  </Field>

  <Field
    label="Silence sensitivity"
    for="vad"
    hint="Higher values treat more quiet audio as silence. Lower this if whispers stop recording too early."
  >
    <Slider
      id="vad"
      min={0}
      max={1}
      step={0.05}
      format={(value) => `${Math.round(value * 100)}%`}
      bind:value={
        () => settings.advanced.vad_sensitivity,
        (value) => settingsStore.patch({ advanced: { vad_sensitivity: value } })
      }
    />
  </Field>
</Section>

<Section
  title="Output & HUD"
  description="Where the floating status appears and how finished text reaches the active app."
>
  <Field label="Show HUD" for="hud" hint="Show recording state, input level, and live preview.">
    <Toggle
      id="hud"
      bind:checked={
        () => settings.dictation.hud,
        (value) => settingsStore.patch({ dictation: { hud: value } })
      }
    />
  </Field>

  {#if capabilities.os === 'macos'}
    <Field
      label="HUD position"
      for="hud-placement"
      hint="Automatic uses the text caret when Accessibility is allowed, then falls back to the pointer."
    >
      <Select
        id="hud-placement"
        options={HUD_PLACEMENT_OPTIONS}
        bind:value={
          () => settings.dictation.hud_placement,
          (value) =>
            settingsStore.patch({ dictation: { hud_placement: value as HudPlacement } })
        }
      />
    </Field>
  {/if}

  <Field
    label="Text insertion"
    for="injection"
    hint="How the final transcript is delivered to the focused application."
  >
    <Select
      id="injection"
      options={availableInjectionOptions}
      bind:value={
        () => settings.advanced.injection,
        (value) =>
          settingsStore.patch({ advanced: { injection: value as InjectionPreference } })
      }
    />
    {#if !configuredInjectionAvailable}
      <p class="warning">
        The configured method is unavailable on {capabilities.os}; choose one of the supported
        methods above.
      </p>
    {/if}
  </Field>

  {#if capabilities.os === 'macos'}
    <Field
      label="Accessibility access"
      hint="Required for caret-aware HUD placement and automatic Command-V insertion."
    >
      {#if permissionError}
        <p class="error">{permissionError}</p>
      {:else if textInjectionPermission === null}
        <p class="muted">Checking…</p>
      {:else}
        <p class:ok={textInjectionPermission === 'granted'}>
          Status: {textInjectionPermission.replace('_', ' ')}
        </p>
        {#if textInjectionPermission === 'not_determined'}
          <button
            type="button"
            onclick={() => requestMacosPermission('text_injection')}
            disabled={permissionBusy !== null}
          >
            {permissionBusy === 'text_injection' ? 'Requesting…' : 'Allow Accessibility'}
          </button>
        {:else if textInjectionPermission === 'denied'}
          <p class="warning">
            Enable Utter in System Settings → Privacy & Security → Accessibility.
          </p>
          {#if textInjectionResetCommand}
            <MacosPermissionRecovery
              kind="text_injection"
              command={textInjectionResetCommand}
              onError={(message) => (permissionError = message)}
            />
          {/if}
          <button type="button" onclick={refreshMacosPermissions}>Check again</button>
        {/if}
      {/if}
    </Field>
  {/if}
</Section>

<Section
  title="Performance & maintenance"
  description="Memory use, updates, and tools for diagnosing a problem."
>
  <Field
    label="Unload idle models"
    for="model-idle-timeout"
    hint="Release memory after a language profile is unused. Its next hotkey press loads it again."
  >
    <Select
      id="model-idle-timeout"
      options={MODEL_IDLE_OPTIONS}
      bind:value={
        () => settings.advanced.model_idle_timeout_secs.toString(),
        (value) =>
          settingsStore.patch({ advanced: { model_idle_timeout_secs: Number(value) } })
      }
    />
  </Field>

  <Field
    label="Updates"
    hint="Release builds verify the manifest and archive signature. Updates are never forced."
  >
    {#if capabilities.updater}
      <button type="button" onclick={checkForUpdate} disabled={updaterBusy !== null}>
        {updaterBusy === 'check' ? 'Checking…' : 'Check for updates'}
      </button>
      {#if updateCheck?.update}
        <div class="update-card">
          <p>
            <strong>Utter {updateCheck.update.version}</strong>
            <span class="muted">Installed: {updateCheck.current_version}</span>
          </p>
          {#if updateCheck.update.notes}
            <p class="update-notes">{updateCheck.update.notes}</p>
          {/if}
          <button type="button" onclick={installUpdate} disabled={updaterBusy !== null}>
            {updaterBusy === 'install' ? 'Installing…' : 'Install and restart'}
          </button>
        </div>
      {/if}
      {#if updaterBusy === 'install'}
        <progress max="100" value={updatePercent ?? undefined}></progress>
        <p class="muted">
          {updatePercent === null ? 'Downloading…' : `Downloaded ${updatePercent}%`}
        </p>
      {/if}
      {#if updateError}
        <p class="error">{updateError}</p>
      {:else if updateMessage}
        <p class="ok">{updateMessage}</p>
      {/if}
    {:else}
      <p class="muted">Update checks are available in signed release builds.</p>
    {/if}
  </Field>

  <Field
    label="Diagnostics"
    hint="The safe report excludes API keys, transcripts, prompts, dictionary terms, endpoints, and personal paths. Nothing is sent automatically."
  >
    <div class="diagnostic-actions">
      <button type="button" onclick={openLogs} disabled={diagnosticsBusy !== null}>
        {diagnosticsBusy === 'open' ? 'Opening…' : 'Open logs'}
      </button>
      <button type="button" onclick={copyDiagnostics} disabled={diagnosticsBusy !== null}>
        {diagnosticsBusy === 'copy' ? 'Copying…' : 'Copy safe report'}
      </button>
    </div>
    {#if diagnosticsError}
      <p class="error">{diagnosticsError}</p>
    {:else if diagnosticsMessage}
      <p class="ok">{diagnosticsMessage}</p>
    {/if}
  </Field>

  <details class="developer-settings">
    <summary>For developers</summary>
    <div class="developer-body">
      <Field label="Log level" for="log-level">
        <Select
          id="log-level"
          options={LOG_LEVEL_OPTIONS}
          bind:value={
            () => settings.advanced.log_level,
            (value) => settingsStore.patch({ advanced: { log_level: value } })
          }
        />
      </Field>
    </div>
  </details>
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
  .muted {
    color: var(--text-muted);
    font-size: 13px;
  }

  .inline-row,
  .diagnostic-actions,
  .meter-row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-2);
  }

  .meter-row {
    flex-direction: column;
    align-items: flex-start;
  }

  .input-meter {
    width: min(100%, 320px);
    height: 8px;
    overflow: hidden;
    border-radius: 999px;
    background: var(--bg-sunken);
  }

  .input-meter span {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--success);
    transition: width 80ms linear;
  }

  .error,
  .warning {
    font-size: 13px;
  }

  .error { color: var(--danger); }
  .warning { color: var(--warning-text); }
  .ok { color: var(--success); }

  button {
    padding: 5px var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text);
    background: var(--bg-elevated);
    cursor: pointer;
    font-size: 13px;
  }

  button:hover:not(:disabled) { background: var(--surface-hover); }
  button:disabled { opacity: 0.55; cursor: not-allowed; }

  .update-card {
    display: grid;
    gap: var(--space-2);
    padding: var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }

  .update-card p {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin: 0;
  }

  .update-notes { white-space: pre-wrap; }
  progress { width: min(100%, 360px); }

  .developer-settings {
    border-top: 1px solid var(--border);
    padding-top: var(--space-3);
  }

  .developer-settings summary {
    color: var(--text-muted);
    cursor: pointer;
    font-size: 13px;
    font-weight: 600;
  }

  .developer-body { padding-top: var(--space-3); }

  @media (prefers-reduced-motion: reduce) {
    .input-meter span { transition: none; }
  }
</style>

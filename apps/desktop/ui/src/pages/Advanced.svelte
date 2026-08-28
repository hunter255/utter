<script lang="ts">
  import { onMount } from 'svelte'

  import Section from '../lib/components/Section.svelte'
  import Field from '../lib/components/Field.svelte'
  import MacosPermissionRecovery from '../lib/components/MacosPermissionRecovery.svelte'
  import Select from '../lib/components/Select.svelte'
  import Slider from '../lib/components/Slider.svelte'
  import * as api from '../lib/api'
  import { settingsStore } from '../lib/stores'
  import type {
    HudPlacement,
    InjectionPreference,
    PermissionKind,
    PermissionStatus,
    PlatformCapabilities,
    UpdateCheck,
    UpdateProgressPayload,
  } from '../lib/types'

  interface Props {
    capabilities: PlatformCapabilities
  }

  let { capabilities }: Props = $props()

  let settings = $derived($settingsStore!)

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
  let availableInjectionOptions = $derived(
    INJECTION_OPTIONS.filter((option) => capabilities.injection_methods.includes(option.value)),
  )
  let configuredInjectionAvailable = $derived(
    capabilities.injection_methods.includes(settings.advanced.injection),
  )

  const LOG_LEVEL_OPTIONS = ['trace', 'debug', 'info', 'warn', 'error'].map((v) => ({
    value: v,
    label: v,
  }))

  const MODEL_IDLE_OPTIONS = [
    { value: '900', label: 'After 15 minutes' },
    { value: '1800', label: 'After 30 minutes' },
    { value: '3600', label: 'After 1 hour' },
    { value: '0', label: 'Never' },
  ]

  let devices = $state<string[]>([])
  let devicesError = $state('')
  let microphonePermission = $state<PermissionStatus | null>(null)
  let microphoneResetCommand = $state('')
  let textInjectionPermission = $state<PermissionStatus | null>(null)
  let textInjectionResetCommand = $state('')
  let permissionBusy = $state<PermissionKind | null>(null)
  let permissionError = $state('')
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

    void refreshMacosPermissions()
    void api
      .listDevices()
      .then((available) => (devices = available))
      .catch((err) => (devicesError = String(err)))

    return () => {
      disposed = true
      unlistenUpdate?.()
    }
  })

  let deviceOptions = $derived([
    { value: '', label: 'System default' },
    ...devices.map((d) => ({ value: d, label: d })),
  ])
</script>

<Section title="Advanced" description="Text injection, audio input, model memory, and diagnostics.">
  <Field label="Text injection" for="injection" hint="How refined text is delivered to the focused app.">
    <Select
      id="injection"
      options={availableInjectionOptions}
      bind:value={
        () => settings.advanced.injection,
        (v) => settingsStore.patch({ advanced: { injection: v as InjectionPreference } })
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
      label="HUD position"
      for="hud-placement"
      hint="Automatic uses the text caret when Accessibility is allowed, then falls back to the pointer."
    >
      <Select
        id="hud-placement"
        options={HUD_PLACEMENT_OPTIONS}
        bind:value={
          () => settings.dictation.hud_placement,
          (v) => settingsStore.patch({ dictation: { hud_placement: v as HudPlacement } })
        }
      />
    </Field>

    <Field
      label="Accessibility permission"
      hint="Lets Utter place the HUD near the caret and send Command-V to the focused field. Without it, the HUD follows the pointer and text stays on the clipboard."
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
            Enable Utter in System Settings → Privacy & Security → Accessibility, then check again.
          </p>
          {#if textInjectionResetCommand}
            <MacosPermissionRecovery
              kind="text_injection"
              command={textInjectionResetCommand}
              onError={(message) => (permissionError = message)}
            />
            <p class="muted">
              Use reset only for a missing or stale entry: copy it, quit Utter, run it in
              Terminal, then reopen Utter and allow access again.
            </p>
          {/if}
          <button type="button" onclick={refreshMacosPermissions}>Check again</button>
        {/if}
      {/if}
    </Field>
  {/if}

  <Field label="Audio input device" for="audio-device">
    {#if devicesError}
      <p class="error">{devicesError}</p>
    {/if}
    <Select
      id="audio-device"
      options={deviceOptions}
      bind:value={
        () => settings.advanced.audio_device ?? '',
        (v) => settingsStore.patch({ advanced: { audio_device: v === '' ? null : v } })
      }
    />
  </Field>

  {#if capabilities.os === 'macos'}
    <Field
      label="Microphone permission"
      hint="macOS should ask once and remember the answer for this signed app identity."
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
            Enable Utter in System Settings → Privacy & Security → Microphone, then check again.
          </p>
          {#if microphoneResetCommand}
            <MacosPermissionRecovery
              kind="microphone"
              command={microphoneResetCommand}
              onError={(message) => (permissionError = message)}
            />
            <p class="muted">
              Use reset only for a missing or stale entry: copy it, quit Utter, run it in
              Terminal, then reopen Utter and allow access again.
            </p>
          {/if}
          <button type="button" onclick={refreshMacosPermissions}>Check again</button>
        {/if}
      {/if}
    </Field>
  {/if}

  <Field
    label="Voice activity sensitivity"
    for="vad"
    hint="Higher values trigger silence detection more eagerly."
  >
    <Slider
      id="vad"
      min={0}
      max={1}
      step={0.05}
      bind:value={
        () => settings.advanced.vad_sensitivity,
        (v) => settingsStore.patch({ advanced: { vad_sensitivity: v } })
      }
    />
  </Field>

  <Field
    label="Unload idle models"
    for="model-idle-timeout"
    hint="Releases memory after a language profile is unused. Its next hotkey press loads it again."
  >
    <Select
      id="model-idle-timeout"
      options={MODEL_IDLE_OPTIONS}
      bind:value={
        () => settings.advanced.model_idle_timeout_secs.toString(),
        (v) =>
          settingsStore.patch({
            advanced: { model_idle_timeout_secs: Number(v) },
          })
      }
    />
  </Field>

  <Field label="Log level" for="log-level">
    <Select
      id="log-level"
      options={LOG_LEVEL_OPTIONS}
      bind:value={
        () => settings.advanced.log_level,
        (v) => settingsStore.patch({ advanced: { log_level: v } })
      }
    />
  </Field>
  <Field
    label="Updates"
    hint="Release builds verify the manifest and archive signature before replacing the application. Updates are never forced."
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
    hint="The report excludes API keys, transcripts, prompts, dictionary terms, endpoints, and personal paths. Nothing is sent automatically."
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
</Section>

<style>
  .error {
    color: var(--danger);
    font-size: 13px;
  }

  .warning {
    color: var(--warning);
    font-size: 13px;
  }

  .muted {
    color: var(--muted);
    font-size: 13px;
  }

  .ok {
    color: var(--success);
  }

  .diagnostic-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }

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

  .update-notes {
    white-space: pre-wrap;
  }

  progress {
    width: min(100%, 360px);
  }

</style>

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
    InjectionPreference,
    PermissionStatus,
    PlatformCapabilities,
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
  let permissionBusy = $state(false)
  let permissionError = $state('')

  async function refreshMicrophonePermission() {
    if (capabilities.os !== 'macos') return
    try {
      const report = await api.permissionsReport()
      if (report.platform === 'macos') {
        microphonePermission = report.microphone
        microphoneResetCommand = report.microphone_reset_command
      }
      permissionError = ''
    } catch (err) {
      permissionError = String(err)
    }
  }

  async function requestMicrophonePermission() {
    permissionBusy = true
    permissionError = ''
    try {
      const report = await api.requestPermission('microphone')
      if (report.platform === 'macos') {
        microphonePermission = report.microphone
        microphoneResetCommand = report.microphone_reset_command
      }
    } catch (err) {
      permissionError = String(err)
    } finally {
      permissionBusy = false
    }
  }

  onMount(async () => {
    void refreshMicrophonePermission()
    try {
      devices = await api.listDevices()
    } catch (err) {
      devicesError = String(err)
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
          <button type="button" onclick={requestMicrophonePermission} disabled={permissionBusy}>
            {permissionBusy ? 'Requesting…' : 'Allow microphone'}
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
          <button type="button" onclick={refreshMicrophonePermission}>Check again</button>
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

</style>

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
  import { formatPercent, t, type MessageKey } from '../lib/i18n'
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

  let themeOptions = $derived<{ value: Theme; label: string }[]>([
    { value: 'system', label: $t('settings.theme.system') },
    { value: 'light', label: $t('settings.theme.light') },
    { value: 'dark', label: $t('settings.theme.dark') },
  ])

  let modeOptions = $derived<{ value: DictationMode; label: string }[]>([
    { value: 'push_to_talk', label: $t('settings.recordingMode.pushToTalk') },
    { value: 'toggle', label: $t('settings.recordingMode.toggle') },
  ])

  let injectionOptions = $derived<{ value: InjectionPreference; label: string }[]>([
    { value: 'auto', label: $t('settings.injection.auto') },
    { value: 'clipboard_paste', label: $t('settings.injection.paste') },
    { value: 'type', label: $t('settings.injection.type') },
    { value: 'clipboard_only', label: $t('settings.injection.clipboard') },
  ])

  let hudPlacementOptions = $derived<{ value: HudPlacement; label: string }[]>([
    { value: 'auto', label: $t('settings.hudPosition.auto') },
    { value: 'pointer', label: $t('settings.hudPosition.pointer') },
    { value: 'bottom_center', label: $t('settings.hudPosition.bottom') },
  ])

  const LOG_LEVEL_OPTIONS = ['trace', 'debug', 'info', 'warn', 'error'].map((value) => ({
    value,
    label: value,
  }))

  let modelIdleOptions = $derived([
    { value: '900', label: $t('settings.modelIdle.15m') },
    { value: '1800', label: $t('settings.modelIdle.30m') },
    { value: '3600', label: $t('settings.modelIdle.1h') },
    { value: '0', label: $t('settings.modelIdle.never') },
  ])

  const PERMISSION_STATUS_KEYS: Record<PermissionStatus, MessageKey> = {
    granted: 'permission.status.granted',
    denied: 'permission.status.denied',
    not_determined: 'permission.status.notDetermined',
    unavailable: 'permission.status.unavailable',
  }

  let availableInjectionOptions = $derived(
    injectionOptions.filter((option) => capabilities.injection_methods.includes(option.value)),
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
    { value: '', label: $t('settings.systemDefault') },
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
      diagnosticsMessage = $t('settings.logsOpened')
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
      diagnosticsMessage = $t('settings.reportCopied')
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
        ? $t('settings.updateAvailable', { version: updateCheck.update.version })
        : $t('settings.upToDate', { version: updateCheck.current_version })
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
    updateMessage = $t('settings.updateDownloading')
    updateProgress = null
    try {
      await api.installUpdate()
      updateMessage = $t('settings.updateInstalled')
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
  <h1>{$t('settings.title')}</h1>
  <p>{$t('settings.description')}</p>
</header>

<Section title={$t('settings.appearance.title')} description={$t('settings.appearance.description')}>
  <Field label={$t('settings.theme')} for="theme">
    <Select
      id="theme"
      options={themeOptions}
      bind:value={
        () => settings.general.theme,
        (value) => settingsStore.patch({ general: { theme: value as Theme } })
      }
    />
  </Field>

  <Field label={$t('settings.launchAtLogin')} for="autostart">
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
  title={$t('settings.recording.title')}
  description={$t('settings.recording.description')}
>
  <Field label={$t('settings.recordingMode')} for="mode">
    <Select
      id="mode"
      options={modeOptions}
      bind:value={
        () => settings.dictation.mode,
        (value) => settingsStore.patch({ dictation: { mode: value as DictationMode } })
      }
    />
  </Field>

  <Field label={$t('settings.silenceTimeout')} hint={$t('settings.silenceTimeoutHint')}>
    <div class="inline-row">
      <Toggle
        id="silence-timeout-enabled"
        bind:checked={() => timeoutEnabled, setTimeoutEnabled}
      />
      <span class="muted">{timeoutEnabled ? $t('common.on') : $t('common.off')}</span>
      {#if timeoutEnabled}
        <TextInput
          type="number"
          min={1}
          max={600}
          bind:value={() => timeoutValue, setTimeoutValue}
        />
        <span class="muted">{$t('settings.seconds')}</span>
      {/if}
    </div>
  </Field>

  <Field label={$t('settings.audioInput')} for="audio-device">
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
      label={$t('settings.microphoneAccess')}
      hint={$t('settings.microphoneAccessHint')}
    >
      {#if permissionError}
        <p class="error">{permissionError}</p>
      {:else if microphonePermission === null}
        <p class="muted">{$t('common.checking')}</p>
      {:else}
        <p class:ok={microphonePermission === 'granted'}>
          {$t('settings.permissionStatus', {
            status: $t(PERMISSION_STATUS_KEYS[microphonePermission]),
          })}
        </p>
        {#if microphonePermission === 'not_determined'}
          <button
            type="button"
            onclick={() => requestMacosPermission('microphone')}
            disabled={permissionBusy !== null}
          >
            {permissionBusy === 'microphone'
              ? $t('common.requesting')
              : $t('settings.allowMicrophone')}
          </button>
        {:else if microphonePermission === 'denied'}
          <p class="warning">{$t('settings.microphoneDenied')}</p>
          {#if microphoneResetCommand}
            <MacosPermissionRecovery
              kind="microphone"
              command={microphoneResetCommand}
              onError={(message) => (permissionError = message)}
            />
          {/if}
          <button type="button" onclick={refreshMacosPermissions}>{$t('common.checkAgain')}</button>
        {/if}
      {/if}
    </Field>
  {/if}

  <Field
    label={$t('settings.liveInput')}
    hint={$t('settings.liveInputHint')}
  >
    <div class="meter-row">
      <div
        class="input-meter"
        role="meter"
        aria-label={$t('hud.microphoneLevel')}
        aria-valuemin="0"
        aria-valuemax="100"
        aria-valuenow={Math.round(inputMeter * 100)}
      >
        <span style:width="{Math.round(inputMeter * 100)}%"></span>
      </div>
      <span class="muted">
        {dictationPhase === 'recording'
          ? inputSignal(inputMeter) === 'voice'
            ? $t('settings.input.voice')
            : inputSignal(inputMeter) === 'quiet'
              ? $t('settings.input.quiet')
              : $t('settings.input.none')
          : $t('settings.input.waiting')}
      </span>
    </div>
  </Field>

  <Field
    label={$t('settings.silenceSensitivity')}
    for="vad"
    hint={$t('settings.silenceSensitivityHint')}
  >
    <Slider
      id="vad"
      min={0}
      max={1}
      step={0.05}
      format={(value) => formatPercent(value)}
      bind:value={
        () => settings.advanced.vad_sensitivity,
        (value) => settingsStore.patch({ advanced: { vad_sensitivity: value } })
      }
    />
  </Field>
</Section>

<Section
  title={$t('settings.output.title')}
  description={$t('settings.output.description')}
>
  <Field label={$t('settings.showHud')} for="hud" hint={$t('settings.showHudHint')}>
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
      label={$t('settings.hudPosition')}
      for="hud-placement"
      hint={$t('settings.hudPositionHint')}
    >
      <Select
        id="hud-placement"
        options={hudPlacementOptions}
        bind:value={
          () => settings.dictation.hud_placement,
          (value) =>
            settingsStore.patch({ dictation: { hud_placement: value as HudPlacement } })
        }
      />
    </Field>
  {/if}

  <Field
    label={$t('settings.textInsertion')}
    for="injection"
    hint={$t('settings.textInsertionHint')}
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
      <p class="warning">{$t('settings.injectionUnavailable', { os: capabilities.os })}</p>
    {/if}
  </Field>

  {#if capabilities.os === 'macos'}
    <Field
      label={$t('settings.accessibility')}
      hint={$t('settings.accessibilityHint')}
    >
      {#if permissionError}
        <p class="error">{permissionError}</p>
      {:else if textInjectionPermission === null}
        <p class="muted">{$t('common.checking')}</p>
      {:else}
        <p class:ok={textInjectionPermission === 'granted'}>
          {$t('settings.permissionStatus', {
            status: $t(PERMISSION_STATUS_KEYS[textInjectionPermission]),
          })}
        </p>
        {#if textInjectionPermission === 'not_determined'}
          <button
            type="button"
            onclick={() => requestMacosPermission('text_injection')}
            disabled={permissionBusy !== null}
          >
            {permissionBusy === 'text_injection'
              ? $t('common.requesting')
              : $t('settings.allowAccessibility')}
          </button>
        {:else if textInjectionPermission === 'denied'}
          <p class="warning">{$t('settings.accessibilityDenied')}</p>
          {#if textInjectionResetCommand}
            <MacosPermissionRecovery
              kind="text_injection"
              command={textInjectionResetCommand}
              onError={(message) => (permissionError = message)}
            />
          {/if}
          <button type="button" onclick={refreshMacosPermissions}>{$t('common.checkAgain')}</button>
        {/if}
      {/if}
    </Field>
  {/if}
</Section>

<Section
  title={$t('settings.performance.title')}
  description={$t('settings.performance.description')}
>
  <Field
    label={$t('settings.unloadModels')}
    for="model-idle-timeout"
    hint={$t('settings.unloadModelsHint')}
  >
    <Select
      id="model-idle-timeout"
      options={modelIdleOptions}
      bind:value={
        () => settings.advanced.model_idle_timeout_secs.toString(),
        (value) =>
          settingsStore.patch({ advanced: { model_idle_timeout_secs: Number(value) } })
      }
    />
  </Field>

  <Field
    label={$t('settings.updates')}
    hint={$t('settings.updatesHint')}
  >
    {#if capabilities.updater}
      <button type="button" onclick={checkForUpdate} disabled={updaterBusy !== null}>
        {updaterBusy === 'check' ? $t('common.checking') : $t('settings.checkUpdates')}
      </button>
      {#if updateCheck?.update}
        <div class="update-card">
          <p>
            <strong>Utter {updateCheck.update.version}</strong>
            <span class="muted">
              {$t('settings.installedVersion', { version: updateCheck.current_version })}
            </span>
          </p>
          {#if updateCheck.update.notes}
            <p class="update-notes">{updateCheck.update.notes}</p>
          {/if}
          <button type="button" onclick={installUpdate} disabled={updaterBusy !== null}>
            {updaterBusy === 'install'
              ? $t('settings.installing')
              : $t('settings.installRestart')}
          </button>
        </div>
      {/if}
      {#if updaterBusy === 'install'}
        <progress max="100" value={updatePercent ?? undefined}></progress>
        <p class="muted">
          {updatePercent === null
            ? $t('settings.downloading')
            : $t('settings.downloadedPercent', { percent: updatePercent })}
        </p>
      {/if}
      {#if updateError}
        <p class="error">{updateError}</p>
      {:else if updateMessage}
        <p class="ok">{updateMessage}</p>
      {/if}
    {:else}
      <p class="muted">{$t('settings.updatesReleaseOnly')}</p>
    {/if}
  </Field>

  <Field
    label={$t('settings.diagnostics')}
    hint={$t('settings.diagnosticsHint')}
  >
    <div class="diagnostic-actions">
      <button type="button" onclick={openLogs} disabled={diagnosticsBusy !== null}>
        {diagnosticsBusy === 'open' ? $t('settings.opening') : $t('settings.openLogs')}
      </button>
      <button type="button" onclick={copyDiagnostics} disabled={diagnosticsBusy !== null}>
        {diagnosticsBusy === 'copy' ? $t('settings.copying') : $t('settings.copyReport')}
      </button>
    </div>
    {#if diagnosticsError}
      <p class="error">{diagnosticsError}</p>
    {:else if diagnosticsMessage}
      <p class="ok">{diagnosticsMessage}</p>
    {/if}
  </Field>

  <details class="developer-settings">
    <summary>{$t('settings.developers')}</summary>
    <div class="developer-body">
      <Field label={$t('settings.logLevel')} for="log-level">
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

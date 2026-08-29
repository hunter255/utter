<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import { getCurrentWindow } from '@tauri-apps/api/window'

  import HotkeyPicker from '../lib/components/HotkeyPicker.svelte'
  import MacosPermissionRecovery from '../lib/components/MacosPermissionRecovery.svelte'
  import ModelInstallAction from '../lib/components/ModelInstallAction.svelte'
  import Select from '../lib/components/Select.svelte'
  import * as api from '../lib/api'
  import { hasBaseKey, parseChordTokens } from '../lib/hotkey'
  import { formatBytes, t, type MessageKey } from '../lib/i18n'
  import { modelStore } from '../lib/model-store'
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

  const STEPS: MessageKey[] = [
    'onboarding.step.welcome',
    'onboarding.step.microphone',
    'onboarding.step.model',
    'onboarding.step.hotkey',
    'onboarding.step.permissions',
    'onboarding.step.done',
  ]
  const PERMISSION_STATUS_KEYS: Record<PermissionStatus, MessageKey> = {
    granted: 'permission.status.granted',
    denied: 'permission.status.denied',
    not_determined: 'permission.status.notDetermined',
    unavailable: 'permission.status.unavailable',
  }
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
  let models = $derived($modelStore.models)
  let modelsError = $derived($modelStore.error)
  let operationBusy = $derived(
    $modelStore.operation !== null || $modelStore.pending !== null,
  )

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
  let languagePickerOptions = $derived(transcriptionLanguageOptions(models, $t))
  let modelPickerOptions = $derived([
    {
      value: '',
      label:
        models.length === 0
          ? $t('onboarding.loadingModels')
          : $t('onboarding.chooseLocalModel'),
    },
    ...transcriptionModelOptions(models, $t),
  ])
  let selectedModel = $derived(
    localModels.find((model) => model.id === profileModelId) ?? null,
  )
  let selectedModelInstalled = $derived(selectedModel?.installed ?? false)
  let languageWarning = $derived(modelLanguageWarning(selectedModel, profileLanguage, $t))

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

  function permissionStatusLabel(status: PermissionStatus): string {
    return $t(PERMISSION_STATUS_KEYS[status])
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
    getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused && permissions?.platform === 'macos') void checkPermissions()
    }).then((fn) => {
      unlistenFocus = fn
    })
  })

  onDestroy(() => {
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
        <span
          class="step-dot"
          class:active={i === step}
          class:done={i < step}
          aria-label={$t(label)}
        >{i + 1}</span>
      {/each}
    </div>

    {#if step === 0}
      <h1>{$t('onboarding.welcomeTitle')}</h1>
      <p>{$t('onboarding.welcomeBody')}</p>
    {:else if step === 1}
      <h1>{$t('onboarding.microphoneTitle')}</h1>
      {#if permissions?.platform === 'macos' && permissions.microphone !== 'granted'}
        <p class="muted">{$t('onboarding.microphoneNeedsAccess')}</p>
        {#if permissions.microphone === 'not_determined'}
          <button
            type="button"
            onclick={() => requestPermission('microphone')}
            disabled={permissionBusy !== null}
          >{permissionBusy === 'microphone'
              ? $t('common.requesting')
              : $t('onboarding.allowMicrophone')}</button>
        {:else if permissions.microphone === 'denied'}
          <p class="warn">{$t('onboarding.microphoneOff')}</p>
          <MacosPermissionRecovery
            kind="microphone"
            command={permissions.microphone_reset_command}
            onError={(message) => (permissionsError = message)}
          />
          <p class="muted">{$t('onboarding.microphoneRecoveryHint')}</p>
        {:else}
          <p class="warn">{$t('onboarding.microphoneUnavailable')}</p>
        {/if}
      {:else}
        <p class="muted">{$t('onboarding.microphoneCheckHint')}</p>
        {#if devicesError}
          <p class="error">{devicesError}</p>
        {:else if devicesChecked}
          {#if devices.length === 0}
            <p class="warn">{$t('onboarding.noInputDevices')}</p>
          {:else}
            <p>
              {$t(
                devices.length === 1
                  ? 'onboarding.foundInputDevice'
                  : 'onboarding.foundInputDevices',
                { count: devices.length },
              )}
            </p>
            <ul>
              {#each devices as device (device)}
                <li>{device}</li>
              {/each}
            </ul>
          {/if}
        {:else}
          <p class="muted">{$t('common.checking')}</p>
        {/if}
      {/if}
    {:else if step === 2}
      <h1>{$t('onboarding.modelTitle')}</h1>
      <p class="muted">{$t('onboarding.modelBody')}</p>
      <div class="picker-field">
        <label for="onboarding-language">{$t('onboarding.language')}</label>
        <Select
          id="onboarding-language"
          options={languagePickerOptions}
          bind:value={() => profileLanguage, selectLanguage}
        />
      </div>
      <div class="picker-field">
        <label for="onboarding-model">{$t('onboarding.model')}</label>
        <Select
          id="onboarding-model"
          options={modelPickerOptions}
          disabled={models.length === 0 || operationBusy}
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
        <p class="muted">{$t('onboarding.cloudProfile')}</p>
      {:else if selectedModel}
        <ul class="model-list">
          <li>
            <div class="model-row">
              <div class="model-info">
                <span class="model-label">{selectedModel.label}</span>
                <span class="model-size">
                  {selectedModel.engine === 'whisper' ? 'Whisper' : 'Sherpa-onnx'} ·
                  {modelCapabilityLabel(selectedModel, $t)} ·
                  {formatBytes(selectedModel.size_mb * 1024 ** 2)}
                </span>
              </div>
            </div>
            <ModelInstallAction
              model={selectedModel}
              beforeInstall={() => settingsStore.flush()}
            />
          </li>
        </ul>
        {#if selectedModelInstalled}
          <p class="ok">{$t('onboarding.modelInstalled')}</p>
        {/if}
      {:else if models.length > 0}
        <p class="warn">{$t('onboarding.chooseModelForLocal')}</p>
      {/if}
    {:else if step === 3}
      <h1>{$t('onboarding.hotkeyTitle')}</h1>
      <p class="muted">{$t('onboarding.hotkeyBody')}</p>
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
            ? $t('onboarding.macosNeedsBaseKey')
            : $t('onboarding.chooseHotkey')}
        </p>
      {/if}
    {:else if step === 4}
      <h1>{$t('onboarding.permissionsTitle')}</h1>
      {#if permissionsError}
        <p class="error">{permissionsError}</p>
      {:else if permissions}
        {#if permissions.platform === 'linux'}
          <p class="muted">{$t('onboarding.linuxPermissionsIntro')}</p>
          <ul class="perm-list">
            <li>
              <span class="perm-status" class:ok={permissions.input_group}>
                {permissions.input_group ? '✓' : '✗'}
              </span>
              {$t('onboarding.inputGroup')}
            </li>
            <li>
              <span class="perm-status" class:ok={permissions.uinput_writable}>
                {permissions.uinput_writable ? '✓' : '✗'}
              </span>
              {$t('onboarding.uinputWritable')}
            </li>
          </ul>
          {#if !permissions.input_group || !permissions.uinput_writable}
            <pre class="fix-command">{permissions.fix_command}</pre>
            <button type="button" onclick={copyFixCommand}>
              {fixCopied ? $t('common.copied') : $t('onboarding.copyFixCommand')}
            </button>
          {:else}
            <p class="ok">{$t('onboarding.allPermissionsAlreadyGranted')}</p>
          {/if}
        {:else if permissions.platform === 'macos'}
          <p class="muted">{$t('onboarding.permissionRequestHint')}</p>
          <ul class="perm-list">
            <li>
              <span class="perm-status" class:ok={permissions.microphone === 'granted'}>
                {permissionMark(permissions.microphone)}
              </span>
              {$t('onboarding.microphonePermission', {
                status: permissionStatusLabel(permissions.microphone),
              })}
              {#if permissions.microphone === 'not_determined'}
                <button
                  type="button"
                  onclick={() => requestPermission('microphone')}
                  disabled={permissionBusy !== null}
                >{permissionBusy === 'microphone'
                    ? $t('common.requesting')
                    : $t('common.allow')}</button>
              {/if}
            </li>
            <li>
              <span class="perm-status" class:ok={permissions.text_injection === 'granted'}>
                {permissionMark(permissions.text_injection)}
              </span>
              {$t('onboarding.pastePermission', {
                status: permissionStatusLabel(permissions.text_injection),
              })}
              {#if permissions.text_injection === 'not_determined'}
                <button
                  type="button"
                  onclick={() => requestPermission('text_injection')}
                  disabled={permissionBusy !== null}
                >{permissionBusy === 'text_injection'
                    ? $t('common.requesting')
                    : $t('common.allow')}</button>
              {/if}
            </li>
          </ul>
          {#if permissions.microphone === 'denied' || permissions.text_injection === 'denied'}
            <p class="warn">{$t('onboarding.deniedPermissionHint')}</p>
            {#if permissions.microphone === 'denied'}
              <strong class="recovery-label">{$t('onboarding.microphoneRecovery')}</strong>
              <MacosPermissionRecovery
                kind="microphone"
                command={permissions.microphone_reset_command}
                onError={(message) => (permissionsError = message)}
              />
            {/if}
            {#if permissions.text_injection === 'denied'}
              <strong class="recovery-label">{$t('onboarding.accessibilityRecovery')}</strong>
              <MacosPermissionRecovery
                kind="text_injection"
                command={permissions.text_injection_reset_command}
                onError={(message) => (permissionsError = message)}
              />
            {/if}
            <p class="muted">{$t('onboarding.resetPermissionHint')}</p>
            <button type="button" onclick={checkPermissions}>{$t('common.checkAgain')}</button>
          {:else if permissions.microphone === 'granted' && permissions.text_injection === 'granted'}
            <p class="ok">{$t('onboarding.allPermissionsGranted')}</p>
          {/if}
        {:else}
          <p class="muted">
            {$t('onboarding.permissionsUnsupported', { os: permissions.os })}
          </p>
        {/if}
      {:else}
        <p class="muted">{$t('common.checking')}</p>
      {/if}
    {:else if step === 5}
      <h1>{$t('onboarding.doneTitle')}</h1>
      <p>{$t('onboarding.doneBody')}</p>
    {/if}

    <div class="actions">
      {#if step > 0}
        <button type="button" onclick={back} disabled={operationBusy}>{$t('common.back')}</button>
      {/if}
      <div class="spacer"></div>
      {#if step < STEPS.length - 1}
        <button type="button" class="ghost" onclick={onDone} disabled={operationBusy}>
          {$t('common.skipSetup')}
        </button>
        <button
          type="button"
          class="primary"
          onclick={next}
          disabled={operationBusy || (step === 3 && !hotkeyValid)}
        >
          {$t('common.continue')}
        </button>
      {:else}
        <button type="button" class="primary" onclick={onDone}>{$t('common.finish')}</button>
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

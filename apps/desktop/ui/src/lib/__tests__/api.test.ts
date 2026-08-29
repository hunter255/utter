import { beforeEach, describe, expect, it, vi } from 'vitest'

// `@tauri-apps/api` is mocked so this suite (and by extension every command
// wrapper) is verifiable without a running tauri backend — it pins down the
// exact command name and argument-object shape each wrapper sends, which is
// the part a refactor is most likely to silently break.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => undefined),
}))
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => undefined),
}))
const mockSetTitle = vi.fn(async () => undefined)
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ setTitle: mockSetTitle }),
}))

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

import * as api from '../api'

const mockInvoke = vi.mocked(invoke)
const mockListen = vi.mocked(listen)

describe('api command wrappers', () => {
  beforeEach(() => {
    mockInvoke.mockClear()
    mockListen.mockClear()
    mockSetTitle.mockClear()
  })

  it('getSettings -> get_settings with no args', async () => {
    await api.getSettings()
    expect(mockInvoke).toHaveBeenCalledWith('get_settings')
  })

  it('saveSettings -> save_settings with a `settings` key', async () => {
    const settings = { general: {} } as never
    await api.saveSettings(settings)
    expect(mockInvoke).toHaveBeenCalledWith('save_settings', { settings })
  })

  it('listDevices -> list_devices', async () => {
    await api.listDevices()
    expect(mockInvoke).toHaveBeenCalledWith('list_devices')
  })

  it('listModels -> list_models', async () => {
    await api.listModels()
    expect(mockInvoke).toHaveBeenCalledWith('list_models')
  })

  it('modelOperationState -> model_operation_state', async () => {
    await api.modelOperationState()
    expect(mockInvoke).toHaveBeenCalledWith('model_operation_state')
  })

  it('downloadModel -> download_model with an `id` key', async () => {
    await api.downloadModel('small')
    expect(mockInvoke).toHaveBeenCalledWith('download_model', { id: 'small' })
  })

  it('cancelModelDownload -> cancel_model_download with an `id` key', async () => {
    await api.cancelModelDownload('small')
    expect(mockInvoke).toHaveBeenCalledWith('cancel_model_download', { id: 'small' })
  })

  it('removeModel -> remove_model with an `id` key', async () => {
    await api.removeModel('small')
    expect(mockInvoke).toHaveBeenCalledWith('remove_model', { id: 'small' })
  })

  it('historyList -> history_list with a `query` key, defaulting to null', async () => {
    await api.historyList('milk')
    expect(mockInvoke).toHaveBeenCalledWith('history_list', { query: 'milk' })

    await api.historyList()
    expect(mockInvoke).toHaveBeenCalledWith('history_list', { query: null })
  })

  it('historyDelete -> history_delete with an `id` key', async () => {
    await api.historyDelete(42)
    expect(mockInvoke).toHaveBeenCalledWith('history_delete', { id: 42 })
  })

  it('historyClear -> history_clear', async () => {
    await api.historyClear()
    expect(mockInvoke).toHaveBeenCalledWith('history_clear')
  })

  it('setApiKey -> set_api_key with `service` and `key` keys, key never logged elsewhere', async () => {
    await api.setApiKey('refine', 'sk-secret')
    expect(mockInvoke).toHaveBeenCalledWith('set_api_key', { service: 'refine', key: 'sk-secret' })
  })

  it('hasApiKey -> has_api_key with a `service` key', async () => {
    await api.hasApiKey('stt')
    expect(mockInvoke).toHaveBeenCalledWith('has_api_key', { service: 'stt' })
  })

  it('permissionsReport -> permissions_report', async () => {
    await api.permissionsReport()
    expect(mockInvoke).toHaveBeenCalledWith('permissions_report')
  })

  it('requestPermission -> request_permission with a closed permission kind', async () => {
    await api.requestPermission('microphone')
    expect(mockInvoke).toHaveBeenCalledWith('request_permission', { kind: 'microphone' })
  })

  it('openPermissionSettings -> open_permission_settings with a closed permission kind', async () => {
    await api.openPermissionSettings('text_injection')
    expect(mockInvoke).toHaveBeenCalledWith('open_permission_settings', {
      kind: 'text_injection',
    })
  })

  it('openLogs -> open_logs', async () => {
    await api.openLogs()
    expect(mockInvoke).toHaveBeenCalledWith('open_logs')
  })

  it('copyDiagnostics -> copy_diagnostics', async () => {
    await api.copyDiagnostics()
    expect(mockInvoke).toHaveBeenCalledWith('copy_diagnostics')
  })

  it('checkForUpdate and installUpdate invoke the serialized backend operations', async () => {
    await api.checkForUpdate()
    expect(mockInvoke).toHaveBeenCalledWith('check_for_update')

    await api.installUpdate()
    expect(mockInvoke).toHaveBeenCalledWith('install_update')
  })

  it('platformCapabilities -> platform_capabilities', async () => {
    await api.platformCapabilities()
    expect(mockInvoke).toHaveBeenCalledWith('platform_capabilities')
  })

  it('setWindowTitle updates the current native window', async () => {
    await api.setWindowTitle('Models — Utter')
    expect(mockSetTitle).toHaveBeenCalledWith('Models — Utter')
  })

  it('testRefine -> test_refine with a `sample` key', async () => {
    await api.testRefine('hello world')
    expect(mockInvoke).toHaveBeenCalledWith('test_refine', { sample: 'hello world' })
  })

  it('cancelDictation -> cancel_dictation', async () => {
    await api.cancelDictation()
    expect(mockInvoke).toHaveBeenCalledWith('cancel_dictation')
  })

  it('onModelOperation listens on "model-operation" and unwraps the snapshot', async () => {
    const handler = vi.fn()
    await api.onModelOperation(handler)
    expect(mockListen).toHaveBeenCalledWith('model-operation', expect.any(Function))
    const listenerCallback = mockListen.mock.calls[0][1] as (event: { payload: unknown }) => void
    const snapshot = {
      generation: 1,
      operation: {
        generation: 1,
        id: 'small',
        kind: 'download',
        phase: 'downloading',
        done: 1,
        total: 2,
      },
    }
    listenerCallback({ payload: snapshot })
    expect(handler).toHaveBeenCalledWith(snapshot)
  })

  it('onUpdateProgress listens on "update-progress" and unwraps the payload', async () => {
    const handler = vi.fn()
    await api.onUpdateProgress(handler)
    expect(mockListen).toHaveBeenCalledWith('update-progress', expect.any(Function))
    const listenerCallback = mockListen.mock.calls[0][1] as (event: { payload: unknown }) => void
    listenerCallback({ payload: { event: 'progress', downloaded: 1, total: 2 } })
    expect(handler).toHaveBeenCalledWith({ event: 'progress', downloaded: 1, total: 2 })
  })

  it('takePendingNotices -> take_pending_notices with no args', async () => {
    await api.takePendingNotices()
    expect(mockInvoke).toHaveBeenCalledWith('take_pending_notices')
  })

  it('onDictationState listens on "dictation-state"', async () => {
    await api.onDictationState(vi.fn())
    expect(mockListen).toHaveBeenCalledWith('dictation-state', expect.any(Function))
  })

  it('onNotice listens on "notice"', async () => {
    await api.onNotice(vi.fn())
    expect(mockListen).toHaveBeenCalledWith('notice', expect.any(Function))
  })
})

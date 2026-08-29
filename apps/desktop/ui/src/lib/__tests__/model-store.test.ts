import { beforeEach, describe, expect, it, vi } from 'vitest'

import { createModelStore, type ModelBackend } from '../model-store'
import type { ModelInfo, ModelOperationSnapshot } from '../types'

function get<T>(store: { subscribe: (fn: (value: T) => void) => () => void }): T {
  let value!: T
  const unsubscribe = store.subscribe((next) => {
    value = next
  })
  unsubscribe()
  return value
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

const MODEL: ModelInfo = {
  id: 'giga',
  engine: 'sherpa',
  label: 'GigaAM',
  size_mb: 500,
  installed: false,
  status: 'missing',
  supported_languages: ['ru'],
  role: 'final',
  performance_class: 'fast',
  recommendation_codes: [],
}

describe('model store', () => {
  let emit!: (snapshot: ModelOperationSnapshot) => void
  let backend: ModelBackend

  beforeEach(() => {
    backend = {
      listModels: vi.fn(async () => [MODEL]),
      modelOperationState: vi.fn(async () => ({ generation: 0, operation: null })),
      onModelOperation: vi.fn(async (handler) => {
        emit = handler
        return () => undefined
      }),
      downloadModel: vi.fn(async () => 'installed' as const),
      cancelModelDownload: vi.fn(async () => undefined),
      removeModel: vi.fn(async () => undefined),
    }
  })

  it('subscribes before asking for the authoritative snapshot', async () => {
    const order: string[] = []
    vi.mocked(backend.onModelOperation).mockImplementation(async (handler) => {
      order.push('listen')
      emit = handler
      return () => undefined
    })
    vi.mocked(backend.modelOperationState).mockImplementation(async () => {
      order.push('snapshot')
      return { generation: 0, operation: null }
    })

    await createModelStore(backend).start()

    expect(order).toEqual(['listen', 'snapshot'])
  })

  it('does not create a second event listener when start is repeated', async () => {
    const store = createModelStore(backend)
    await Promise.all([store.start(), store.start()])
    await store.start()

    expect(backend.onModelOperation).toHaveBeenCalledTimes(1)
  })

  it('restores an in-flight download and ignores stale completion events', async () => {
    vi.mocked(backend.modelOperationState).mockResolvedValue({
      generation: 4,
      operation: {
        generation: 4,
        id: 'giga',
        kind: 'download',
        phase: 'downloading',
        done: 25,
        total: 100,
      },
    })
    const store = createModelStore(backend)
    await store.start()

    expect(get(store).operation?.done).toBe(25)
    emit({
      generation: 4,
      operation: {
        generation: 4,
        id: 'giga',
        kind: 'download',
        phase: 'preparing',
        done: 10,
        total: 100,
      },
    })
    expect(get(store).operation?.done).toBe(25)
    expect(get(store).operation?.phase).toBe('downloading')

    emit({ generation: 3, operation: null })
    expect(get(store).operation?.id).toBe('giga')

    emit({ generation: 4, operation: null })
    expect(get(store).operation).toBeNull()

    emit({
      generation: 4,
      operation: {
        generation: 4,
        id: 'giga',
        kind: 'download',
        phase: 'downloading',
        done: 99,
        total: 100,
      },
    })
    expect(get(store).operation).toBeNull()
  })

  it('keeps the pending action in the app-wide store until backend events take over', async () => {
    const download = deferred<'installed'>()
    vi.mocked(backend.downloadModel).mockReturnValue(download.promise)
    const store = createModelStore(backend)
    await store.start()

    const installing = store.install('giga')
    expect(get(store).pending).toEqual({ id: 'giga', kind: 'download' })

    emit({
      generation: 1,
      operation: {
        generation: 1,
        id: 'giga',
        kind: 'download',
        phase: 'downloading',
        done: 10,
        total: 100,
      },
    })
    expect(get(store).pending).toBeNull()
    expect(get(store).operation?.done).toBe(10)

    emit({ generation: 1, operation: null })
    download.resolve('installed')
    await installing
    expect(get(store).operation).toBeNull()
    expect(backend.listModels).toHaveBeenCalledTimes(2)
  })

  it('reserves the global operation while settings are prepared before download', async () => {
    const prepare = deferred<void>()
    const store = createModelStore(backend)
    await store.start()

    const installing = store.install('giga', () => prepare.promise)
    expect(get(store).pending).toEqual({ id: 'giga', kind: 'download' })
    expect(backend.downloadModel).not.toHaveBeenCalled()

    prepare.resolve()
    await installing
    expect(backend.downloadModel).toHaveBeenCalledWith('giga')
  })

  it('does not start a download when saving the selected model fails', async () => {
    const store = createModelStore(backend)
    await store.start()

    await store.install('giga', async () => {
      throw new Error('settings are read-only')
    })

    expect(backend.downloadModel).not.toHaveBeenCalled()
    expect(get(store).pending).toBeNull()
    expect(get(store).error).toContain('settings are read-only')
  })

  it('marks cancellation immediately and restores the prior phase on failure', async () => {
    const store = createModelStore(backend)
    await store.start()
    emit({
      generation: 1,
      operation: {
        generation: 1,
        id: 'giga',
        kind: 'download',
        phase: 'downloading',
        done: 10,
        total: 100,
      },
    })
    vi.mocked(backend.cancelModelDownload).mockRejectedValue(new Error('offline'))

    await store.cancel('giga')

    expect(get(store).operation?.phase).toBe('downloading')
    expect(get(store).error).toContain('offline')
  })
})

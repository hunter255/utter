// One app-wide owner for the model catalog and its single file operation.
// Pages are mounted and destroyed as the settings hash changes; keeping this
// state outside them means a download remains visible and cancellable after
// navigation, while the backend snapshot restores it after a webview reload.

import { writable, type Readable } from 'svelte/store'
import type { UnlistenFn } from '@tauri-apps/api/event'

import * as api from './api'
import type {
  ModelInfo,
  ModelOperationKind,
  ModelOperationSnapshot,
  ModelOperationState,
} from './types'

export interface PendingModelOperation {
  id: string
  kind: ModelOperationKind
}

export interface ModelStoreState {
  models: ModelInfo[]
  operation: ModelOperationState | null
  pending: PendingModelOperation | null
  generation: number
  loading: boolean
  error: string
}

export type ModelBackend = Pick<
  typeof api,
  | 'listModels'
  | 'modelOperationState'
  | 'onModelOperation'
  | 'downloadModel'
  | 'cancelModelDownload'
  | 'removeModel'
>

export interface ModelStore extends Readable<ModelStoreState> {
  start(): Promise<void>
  stop(): void
  refresh(): Promise<void>
  install(id: string): Promise<void>
  cancel(id: string): Promise<void>
  remove(id: string): Promise<void>
}

const INITIAL_STATE: ModelStoreState = {
  models: [],
  operation: null,
  pending: null,
  generation: 0,
  loading: true,
  error: '',
}

export function createModelStore(backend: ModelBackend = api): ModelStore {
  const { subscribe, update } = writable<ModelStoreState>({ ...INITIAL_STATE })
  let unlisten: UnlistenFn | undefined
  let starting: Promise<void> | null = null
  let current: ModelStoreState = { ...INITIAL_STATE }
  let completedGeneration = 0

  const remember = (next: ModelStoreState): ModelStoreState => {
    current = next
    return next
  }

  function setError(error: string): void {
    update((state) => remember({ ...state, error }))
  }

  async function refresh(): Promise<void> {
    try {
      const models = await backend.listModels()
      update((state) => remember({ ...state, models, loading: false, error: '' }))
    } catch (error) {
      update((state) => remember({ ...state, loading: false, error: String(error) }))
    }
  }

  function applySnapshot(snapshot: ModelOperationSnapshot): void {
    if (snapshot.operation && snapshot.generation <= completedGeneration) return
    if (!snapshot.operation) {
      completedGeneration = Math.max(completedGeneration, snapshot.generation)
    }
    let completed = false
    update((state) => {
      if (snapshot.generation < state.generation) return remember(state)
      if (
        snapshot.generation === state.generation &&
        snapshot.operation &&
        state.operation &&
        (snapshot.operation.done < state.operation.done ||
          phaseRank(snapshot.operation.phase) < phaseRank(state.operation.phase))
      ) {
        return remember(state)
      }
      completed = state.operation !== null && snapshot.operation === null
      return remember({
        ...state,
        generation: snapshot.generation,
        operation: snapshot.operation,
        pending: snapshot.operation ? null : state.pending,
      })
    })
    if (completed) void refresh()
  }

  function phaseRank(phase: ModelOperationState['phase']): number {
    if (phase === 'preparing') return 0
    if (phase === 'downloading' || phase === 'removing') return 1
    return 2 // cancelling is terminal until the worker acknowledges it
  }

  async function start(): Promise<void> {
    if (starting) return starting
    starting = (async () => {
      try {
        // Subscribe first, then fetch the snapshot. Generation checks make a
        // delayed older response harmless if an event wins the race.
        unlisten = await backend.onModelOperation(applySnapshot)
        const [snapshot] = await Promise.all([backend.modelOperationState(), refresh()])
        applySnapshot(snapshot)
      } catch (error) {
        setError(`Failed to restore model downloads: ${String(error)}`)
      }
    })()
    return starting
  }

  function stop(): void {
    unlisten?.()
    unlisten = undefined
    starting = null
  }

  function reserve(id: string, kind: ModelOperationKind): boolean {
    if (current.operation || current.pending) return false
    update((state) => remember({ ...state, pending: { id, kind }, error: '' }))
    return true
  }

  async function settleOperation(startGeneration: number): Promise<void> {
    // Normally the completion event has already cleared the operation and
    // refreshed the catalog before the command resolves. If an event was
    // missed, reconcile from the backend; if no event arrived at all, at
    // least refresh the catalog so the installed flag cannot stay stale.
    if (current.operation) {
      try {
        applySnapshot(await backend.modelOperationState())
      } catch (error) {
        setError(`Failed to restore model downloads: ${String(error)}`)
      }
    }
    if (current.generation === startGeneration) await refresh()
  }

  function releasePending(id: string): void {
    update((state) =>
      remember({
        ...state,
        pending: state.pending?.id === id ? null : state.pending,
      }),
    )
  }

  async function install(id: string): Promise<void> {
    if (!reserve(id, 'download')) return
    const startGeneration = current.generation
    try {
      await backend.downloadModel(id)
      await settleOperation(startGeneration)
    } catch (error) {
      setError(`Failed to download "${id}": ${String(error)}`)
    } finally {
      releasePending(id)
    }
  }

  async function cancel(id: string): Promise<void> {
    if (current.operation?.id !== id || current.operation.kind !== 'download') return
    const previous = current.operation
    update((state) =>
      remember({
        ...state,
        operation: state.operation ? { ...state.operation, phase: 'cancelling' } : null,
        error: '',
      }),
    )
    try {
      await backend.cancelModelDownload(id)
    } catch (error) {
      update((state) =>
        remember({
          ...state,
          operation:
            state.operation?.generation === previous.generation ? previous : state.operation,
          error: `Failed to cancel "${id}": ${String(error)}`,
        }),
      )
    }
  }

  async function remove(id: string): Promise<void> {
    if (!reserve(id, 'remove')) return
    const startGeneration = current.generation
    try {
      await backend.removeModel(id)
      await settleOperation(startGeneration)
    } catch (error) {
      setError(`Failed to remove "${id}": ${String(error)}`)
    } finally {
      releasePending(id)
    }
  }

  return { subscribe, start, stop, refresh, install, cancel, remove }
}

export const modelStore = createModelStore()

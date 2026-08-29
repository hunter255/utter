<script lang="ts">
  import { onMount } from 'svelte'

  import Section from '../lib/components/Section.svelte'
  import Field from '../lib/components/Field.svelte'
  import TextInput from '../lib/components/TextInput.svelte'
  import Toggle from '../lib/components/Toggle.svelte'
  import * as api from '../lib/api'
  import { formatDateTime, formatDuration, t } from '../lib/i18n'
  import { settingsStore } from '../lib/stores'
  import type { HistoryEntry } from '../lib/types'

  let settings = $derived($settingsStore!)

  let query = $state('')
  let entries = $state<HistoryEntry[]>([])
  let loadError = $state('')
  let confirmingClear = $state(false)
  let copiedId = $state<number | null>(null)
  let searchDebounce: ReturnType<typeof setTimeout> | null = null

  async function refresh() {
    try {
      entries = await api.historyList(query || undefined)
      loadError = ''
    } catch (err) {
      loadError = String(err)
    }
  }

  onMount(refresh)

  function onQueryInput() {
    if (searchDebounce) clearTimeout(searchDebounce)
    searchDebounce = setTimeout(refresh, 250)
  }

  async function copy(entry: HistoryEntry) {
    try {
      await navigator.clipboard.writeText(entry.final_text)
      copiedId = entry.id
      setTimeout(() => {
        if (copiedId === entry.id) copiedId = null
      }, 1500)
    } catch {
      // Clipboard access can be denied by the OS/browser sandbox; leave the
      // row's state alone rather than throwing in the UI.
    }
  }

  async function remove(id: number) {
    await api.historyDelete(id)
    await refresh()
  }

  async function clearAll() {
    await api.historyClear()
    confirmingClear = false
    await refresh()
  }
</script>

<Section title={$t('history.title')} description={$t('history.description')}>
  <Field label={$t('history.record')} for="history-enabled">
    <Toggle
      id="history-enabled"
      bind:checked={
        () => settings.history.enabled,
        (v) => settingsStore.patch({ history: { enabled: v } })
      }
    />
  </Field>

  {#if !settings.history.enabled}
    <p class="note">{$t('history.disabledHint')}</p>
  {/if}

  <div class="toolbar">
    <TextInput
      placeholder={$t('history.search')}
      bind:value={
        () => query,
        (v) => {
          query = v
          onQueryInput()
        }
      }
    />
    {#if entries.length > 0}
      {#if confirmingClear}
        <span class="confirm">
          {$t(
            entries.length === 1
              ? 'history.clearConfirm.one'
              : 'history.clearConfirm.other',
            { count: entries.length },
          )}
          <button type="button" class="danger" onclick={clearAll}>{$t('common.confirm')}</button>
          <button type="button" onclick={() => (confirmingClear = false)}>{$t('common.cancel')}</button>
        </span>
      {:else}
        <button type="button" onclick={() => (confirmingClear = true)}>{$t('history.clearAll')}</button>
      {/if}
    {/if}
  </div>

  {#if loadError}
    <p class="error">{loadError}</p>
  {:else if entries.length === 0}
    <p class="note">{$t(query ? 'history.emptySearch' : 'history.empty')}</p>
  {:else}
    <ul class="entries">
      {#each entries as entry (entry.id)}
        <li>
          <div class="entry-text">{entry.final_text}</div>
          <div class="entry-meta">
            <span>{formatDateTime(entry.created_at * 1000)}</span>
            <span>{entry.engine}</span>
            {#if entry.app}<span>{entry.app}</span>{/if}
            <span>{formatDuration(entry.duration_ms)}</span>
          </div>
          <div class="entry-actions">
            <button type="button" onclick={() => copy(entry)}>
              {copiedId === entry.id ? $t('common.copied') : $t('common.copy')}
            </button>
            <button type="button" class="danger" onclick={() => remove(entry.id)}>
              {$t('common.delete')}
            </button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</Section>

<style>
  .note {
    font-size: 13px;
    color: var(--text-muted);
  }

  .error {
    font-size: 13px;
    color: var(--danger);
  }

  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .confirm {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: 13px;
    color: var(--text-muted);
  }

  .entries {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .entries li {
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .entry-text {
    font-size: 13px;
  }

  .entry-meta {
    display: flex;
    gap: var(--space-3);
    font-size: 12px;
    color: var(--text-muted);
  }

  .entry-actions {
    display: flex;
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

  button.danger {
    color: var(--danger);
  }
</style>

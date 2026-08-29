<script lang="ts">
  import Section from '../lib/components/Section.svelte'
  import { modelStore } from '../lib/model-store'
  import { modelCapabilityLabel, previewModels } from '../lib/models'

  let models = $derived($modelStore.models)
  let modelsError = $derived($modelStore.error)
  let operation = $derived($modelStore.operation)
  let pending = $derived($modelStore.pending)
  let activeDownloadId = $derived(
    operation?.kind === 'download'
      ? operation.id
      : pending?.kind === 'download'
        ? pending.id
        : null,
  )
  let activeRemoveId = $derived(
    operation?.kind === 'remove'
      ? operation.id
      : pending?.kind === 'remove'
        ? pending.id
        : null,
  )
  let cancellingDownload = $derived(operation?.phase === 'cancelling')
  let operationBusy = $derived(operation !== null || pending !== null)
  let whisperModels = $derived(models.filter((m) => m.role === 'final' && m.engine === 'whisper'))
  let sherpaModels = $derived(models.filter((m) => m.role === 'final' && m.engine === 'sherpa'))
  let streamingModels = $derived(previewModels(models))

  function progressPercent(id: string): number | null {
    const p = operation?.id === id ? operation : null
    if (!p || p.total <= 0) return null
    return Math.min(100, Math.round((p.done / p.total) * 100))
  }

  async function install(id: string) {
    await modelStore.install(id)
  }

  async function cancelDownload(id: string) {
    await modelStore.cancel(id)
  }

  async function remove(id: string) {
    await modelStore.remove(id)
  }

</script>

<header class="page-heading">
  <h1>Models</h1>
  <p>Install and remove local transcription and live-preview resources.</p>
</header>

<Section title="Whisper models" description="Runs fully offline. Larger models are more accurate but slower. Which model a profile uses is set on the Profiles page.">
  {#if modelsError}
    <p class="error">{modelsError}</p>
  {/if}
  <ul class="model-list">
    {#each whisperModels as model (model.id)}
      <li>
        <div class="model-row">
          <div class="model-info">
            <span class="model-label">{model.label}</span>
            <span class="model-size">{modelCapabilityLabel(model)} · {model.size_mb} MB</span>
          </div>
          <div class="model-actions">
            {#if activeRemoveId === model.id}
              <button type="button" disabled>Removing…</button>
            {:else if activeDownloadId === model.id}
              <button
                type="button"
                class="cancel"
                onclick={() => cancelDownload(model.id)}
                disabled={cancellingDownload}
              >{cancellingDownload ? 'Cancelling…' : 'Cancel'}</button>
            {:else if model.status === 'ready'}
              <span class="badge badge-installed">Installed</span>
              <button type="button" onclick={() => remove(model.id)} disabled={operationBusy}>
                Remove
              </button>
            {:else}
              {#if model.status === 'damaged'}
                <span class="badge badge-damaged">Damaged</span>
              {/if}
              <button type="button" onclick={() => install(model.id)} disabled={operationBusy}>
                {model.status === 'damaged' ? 'Re-download' : 'Install'}
              </button>
            {/if}
          </div>
        </div>
        {#if activeDownloadId === model.id}
          <div class="progress-track" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={progressPercent(model.id) ?? undefined}>
            <div class="progress-fill" style:width="{progressPercent(model.id) ?? 0}%"></div>
          </div>
        {/if}
      </li>
    {/each}
  </ul>
</Section>

<Section title="Sherpa-onnx models" description="Offline transducer models, one per language, that emit punctuation directly and bias recognition towards your dictionary terms.">
  <ul class="model-list">
    {#each sherpaModels as model (model.id)}
      <li>
        <div class="model-row">
          <div class="model-info">
            <span class="model-label">{model.label}</span>
            <span class="model-size">{modelCapabilityLabel(model)} · {model.size_mb} MB</span>
          </div>
          <div class="model-actions">
            {#if activeRemoveId === model.id}
              <button type="button" disabled>Removing…</button>
            {:else if activeDownloadId === model.id}
              <button
                type="button"
                class="cancel"
                onclick={() => cancelDownload(model.id)}
                disabled={cancellingDownload}
              >{cancellingDownload ? 'Cancelling…' : 'Cancel'}</button>
            {:else if model.status === 'ready'}
              <span class="badge badge-installed">Installed</span>
              <button type="button" onclick={() => remove(model.id)} disabled={operationBusy}>
                Remove
              </button>
            {:else}
              {#if model.status === 'damaged'}
                <span class="badge badge-damaged">Damaged</span>
              {/if}
              <button type="button" onclick={() => install(model.id)} disabled={operationBusy}>
                {model.status === 'damaged' ? 'Re-download' : 'Install'}
              </button>
            {/if}
          </div>
        </div>
        {#if activeDownloadId === model.id}
          <div class="progress-track" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={progressPercent(model.id) ?? undefined}>
            <div class="progress-fill" style:width="{progressPercent(model.id) ?? 0}%"></div>
          </div>
        {/if}
      </li>
    {/each}
  </ul>
</Section>

<Section title="Live preview models" description="Streaming models show provisional words in the HUD while you speak. They trade size, latency, and accuracy differently and may omit punctuation; their text is never inserted. Choose one per profile on the Profiles page.">
  <ul class="model-list">
    {#each streamingModels as model (model.id)}
      <li>
        <div class="model-row">
          <div class="model-info">
            <span class="model-label">{model.label}</span>
            <span class="model-size">{modelCapabilityLabel(model)} · {model.size_mb} MB</span>
          </div>
          <div class="model-actions">
            {#if activeRemoveId === model.id}
              <button type="button" disabled>Removing…</button>
            {:else if activeDownloadId === model.id}
              <button
                type="button"
                class="cancel"
                onclick={() => cancelDownload(model.id)}
                disabled={cancellingDownload}
              >{cancellingDownload ? 'Cancelling…' : 'Cancel'}</button>
            {:else if model.status === 'ready'}
              <span class="badge badge-installed">Installed</span>
              <button type="button" onclick={() => remove(model.id)} disabled={operationBusy}>
                Remove
              </button>
            {:else}
              {#if model.status === 'damaged'}
                <span class="badge badge-damaged">Damaged</span>
              {/if}
              <button type="button" onclick={() => install(model.id)} disabled={operationBusy}>
                {model.status === 'damaged' ? 'Re-download' : 'Install'}
              </button>
            {/if}
          </div>
        </div>
        {#if activeDownloadId === model.id}
          <div class="progress-track" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={progressPercent(model.id) ?? undefined}>
            <div class="progress-fill" style:width="{progressPercent(model.id) ?? 0}%"></div>
          </div>
        {/if}
      </li>
    {/each}
  </ul>
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

  .page-heading p {
    color: var(--text-muted);
    font-size: 13px;
  }

  .error {
    color: var(--danger);
    font-size: 13px;
  }

  .model-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .model-list li {
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
  }

  .model-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .model-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .model-label {
    font-size: 13px;
    font-weight: 500;
  }

  .model-size {
    font-size: 12px;
    color: var(--text-muted);
  }

  .model-actions {
    display: flex;
    align-items: center;
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

  button:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  button.cancel {
    color: var(--danger);
  }

  .badge {
    font-size: 11px;
    font-weight: 600;
    padding: 2px var(--space-2);
    border-radius: 999px;
  }

  .badge-installed {
    background: var(--success);
    color: var(--accent-contrast);
  }

  .badge-damaged {
    background: var(--danger);
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

</style>

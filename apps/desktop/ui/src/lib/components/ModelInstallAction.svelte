<script lang="ts">
  import { modelStore } from '../model-store'
  import { t } from '../i18n'
  import type { ModelInfo } from '../types'

  interface Props {
    model: ModelInfo | null
    beforeInstall?: () => Promise<void>
  }

  let { model, beforeInstall }: Props = $props()
  let operation = $derived($modelStore.operation?.id === model?.id ? $modelStore.operation : null)
  let pending = $derived($modelStore.pending?.id === model?.id ? $modelStore.pending : null)
  let busy = $derived($modelStore.operation !== null || $modelStore.pending !== null)
  let downloading = $derived(
    operation?.kind === 'download' || pending?.kind === 'download',
  )
  let percent = $derived(
    operation && operation.total > 0
      ? Math.min(100, Math.round((operation.done / operation.total) * 100))
      : null,
  )
  let downloadLabel = $derived(
    model?.status === 'damaged'
      ? $t('model.redownloadSize', { size: model.size_mb })
      : $t('model.downloadSizeAndUse', { size: model?.size_mb ?? 0 }),
  )
</script>

{#if model}
  <div class="install-action">
    <div class="status-row">
      {#if model.status === 'ready'}
        <span class="badge ready">{$t('model.readyToUse')}</span>
      {:else if downloading}
        <span class="state">
          {operation?.phase === 'cancelling'
            ? $t('common.cancelling')
            : percent === null
              ? $t('model.preparingDownload')
              : $t('model.downloadingPercent', { percent })}
        </span>
        {#if operation?.kind === 'download'}
          <button
            type="button"
            class="cancel"
            onclick={() => modelStore.cancel(model.id)}
            disabled={operation.phase === 'cancelling'}
          >{operation.phase === 'cancelling' ? $t('common.cancelling') : $t('common.cancel')}</button>
        {/if}
      {:else}
        <span class:damaged={model.status === 'damaged'} class="state">
          {model.status === 'damaged'
            ? $t('model.filesDamaged')
            : $t('model.selectedNotInstalled')}
        </span>
        <button
          type="button"
          onclick={() => modelStore.install(model.id, beforeInstall)}
          disabled={busy}
        >{downloadLabel}</button>
      {/if}
    </div>

    {#if downloading}
      <div
        class="progress-track"
        class:indeterminate={percent === null}
        role="progressbar"
        aria-label={$t('model.downloadAria', { model: model.label })}
        aria-valuemin="0"
        aria-valuemax="100"
        aria-valuenow={percent ?? undefined}
      >
        <div class="progress-fill" style:width="{percent ?? 28}%"></div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .install-action {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-sunken);
  }

  .status-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .state {
    color: var(--text-muted);
    font-size: 12px;
  }

  .state.damaged {
    color: var(--danger);
    font-weight: 600;
  }

  .badge {
    padding: 2px var(--space-2);
    border-radius: 999px;
    font-size: 11px;
    font-weight: 600;
  }

  .ready {
    background: var(--success);
    color: var(--accent-contrast);
  }

  button {
    padding: 5px var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-elevated);
    cursor: pointer;
    font-size: 12px;
  }

  button.cancel { color: var(--danger); }
  button:disabled { opacity: 0.55; cursor: not-allowed; }

  .progress-track {
    height: 5px;
    overflow: hidden;
    border-radius: 999px;
    background: var(--bg);
  }

  .progress-fill {
    height: 100%;
    border-radius: inherit;
    background: var(--accent);
    transition: width 150ms ease;
  }

  .indeterminate .progress-fill {
    animation: slide 1.1s ease-in-out infinite alternate;
  }

  @keyframes slide {
    from { transform: translateX(-20%); }
    to { transform: translateX(280%); }
  }

  @media (prefers-reduced-motion: reduce) {
    .indeterminate .progress-fill { animation: none; }
  }
</style>

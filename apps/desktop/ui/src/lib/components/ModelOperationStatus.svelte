<script lang="ts">
  import { modelStore } from '../model-store'

  let active = $derived(
    $modelStore.operation ??
      ($modelStore.pending
        ? {
            generation: $modelStore.generation,
            id: $modelStore.pending.id,
            kind: $modelStore.pending.kind,
            phase: $modelStore.pending.kind === 'download' ? 'preparing' : 'removing',
            done: 0,
            total: 0,
          }
        : null),
  )
  let modelLabel = $derived(
    $modelStore.models.find((model) => model.id === active?.id)?.label ?? active?.id ?? '',
  )
  let percent = $derived(
    active && active.total > 0
      ? Math.min(100, Math.round((active.done / active.total) * 100))
      : null,
  )
  let status = $derived(
    active?.phase === 'cancelling'
      ? 'Cancelling…'
      : active?.kind === 'remove'
        ? 'Removing…'
        : percent === null
          ? 'Preparing download…'
          : `Downloading ${percent}%`,
  )
</script>

{#if active}
  <aside class="model-operation" aria-live="polite" aria-label="Model operation">
    <div class="copy">
      <strong>{modelLabel}</strong>
      <span>{status}</span>
    </div>
    {#if $modelStore.operation?.kind === 'download'}
      <button
        type="button"
        onclick={() => modelStore.cancel($modelStore.operation!.id)}
        disabled={active.phase === 'cancelling'}
      >{active.phase === 'cancelling' ? 'Cancelling…' : 'Cancel'}</button>
    {/if}
    {#if active.kind === 'download'}
      <div
        class="track"
        class:indeterminate={percent === null}
        role="progressbar"
        aria-valuemin="0"
        aria-valuemax="100"
        aria-valuenow={percent ?? undefined}
      >
        <div class="fill" style:width="{percent ?? 28}%"></div>
      </div>
    {/if}
  </aside>
{/if}

<style>
  .model-operation {
    position: fixed;
    top: var(--space-3);
    right: var(--space-3);
    z-index: 30;
    width: min(320px, calc(100vw - 24px));
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: var(--space-2) var(--space-3);
    align-items: center;
    padding: var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-elevated);
    box-shadow: 0 10px 30px rgb(0 0 0 / 16%);
  }

  .copy {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  strong,
  span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  strong {
    font-size: 13px;
  }

  span {
    color: var(--text-muted);
    font-size: 12px;
  }

  button {
    padding: 5px var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--danger);
    cursor: pointer;
  }

  button:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .track {
    grid-column: 1 / -1;
    height: 5px;
    overflow: hidden;
    border-radius: 999px;
    background: var(--bg-sunken);
  }

  .fill {
    height: 100%;
    border-radius: inherit;
    background: var(--accent);
    transition: width 150ms ease;
  }

  .indeterminate .fill {
    animation: slide 1.1s ease-in-out infinite alternate;
  }

  @keyframes slide {
    from { transform: translateX(-20%); }
    to { transform: translateX(280%); }
  }

  @media (prefers-reduced-motion: reduce) {
    .indeterminate .fill { animation: none; }
  }
</style>

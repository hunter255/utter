<script lang="ts">
  import type { Snippet } from 'svelte'
  import { t } from '../i18n'

  interface Props {
    title: string
    summary: string
    ready: boolean
    expanded: boolean
    onToggle: () => void
    children?: Snippet
  }

  let { title, summary, ready, expanded, onToggle, children }: Props = $props()
</script>

<section class="profile-card" class:expanded>
  <button
    type="button"
    class="profile-header"
    aria-expanded={expanded}
    onclick={onToggle}
  >
    <span class="header-copy">
      <strong>{title}</strong>
      <span class="summary">{summary}</span>
    </span>
    <span class:ready class:needs-setup={!ready} class="readiness">
      {ready ? $t('common.ready') : $t('common.needsSetup')}
    </span>
    <span class="chevron" aria-hidden="true">⌄</span>
  </button>

  {#if expanded}
    <div class="profile-body">
      {#if children}{@render children()}{/if}
    </div>
  {/if}
</section>

<style>
  .profile-card {
    overflow: hidden;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-elevated);
  }

  .profile-card.expanded {
    border-color: color-mix(in srgb, var(--accent) 35%, var(--border));
  }

  .profile-header {
    width: 100%;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-4);
    border: 0;
    background: transparent;
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }

  .profile-header:hover { background: var(--bg-sunken); }

  .header-copy {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  strong {
    font-size: 14px;
    font-weight: 650;
  }

  .summary {
    overflow: hidden;
    color: var(--text-muted);
    font-size: 12px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .readiness {
    padding: 2px var(--space-2);
    border-radius: 999px;
    font-size: 11px;
    font-weight: 650;
  }

  .readiness.ready {
    background: var(--success);
    color: var(--accent-contrast);
  }

  .readiness.needs-setup {
    background: var(--warning-bg);
    color: var(--warning-text);
  }

  .chevron {
    color: var(--text-muted);
    font-size: 18px;
    transform: rotate(-90deg);
    transition: transform 120ms ease;
  }

  [aria-expanded='true'] .chevron { transform: rotate(0); }

  .profile-body {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding: 0 var(--space-4) var(--space-4);
    border-top: 1px solid var(--border);
  }

  @media (prefers-reduced-motion: reduce) {
    .chevron { transition: none; }
  }
</style>

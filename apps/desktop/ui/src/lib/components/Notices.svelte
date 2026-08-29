<script lang="ts">
  // Renders whatever is currently in `noticeStore` (see `lib/notices.ts` for
  // why this window is the notice's second stop, not its first).
  import { noticeDisplayMessage, noticeStore } from '../notices'
  import { t, type MessageKey } from '../i18n'

  const KIND_LABEL: Record<'info' | 'warning' | 'error', MessageKey> = {
    info: 'notice.info',
    warning: 'notice.warning',
    error: 'notice.error',
  } as const
</script>

{#if $noticeStore.length > 0}
  <div class="notices" role="status" aria-live="polite">
    {#each $noticeStore as notice (notice.id)}
      <div class="notice" data-kind={notice.kind}>
        <div class="text">
          <div class="head">
            <span class="kind">{$t(KIND_LABEL[notice.kind])}</span>
            {#if notice.count > 1}
              <span class="count">×{notice.count}</span>
            {/if}
          </div>
          <p class="message">{noticeDisplayMessage(notice, $t)}</p>
          {#if notice.detail}
            <details>
              <summary>{$t('notice.technicalDetails')}</summary>
              <p class="detail">{notice.detail}</p>
            </details>
          {/if}
        </div>
        <button
          type="button"
          class="dismiss"
          aria-label={$t('notice.dismiss')}
          onclick={() => noticeStore.dismiss(notice.id)}
        >
          ×
        </button>
      </div>
    {/each}
  </div>
{/if}

<style>
  .notices {
    position: fixed;
    right: var(--space-4);
    bottom: var(--space-4);
    z-index: 10;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-width: 380px;
  }

  .notice {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
    padding: var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-elevated);
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.18);
    font-size: 13px;
  }

  .notice[data-kind='warning'] {
    background: var(--warning-bg);
    color: var(--warning-text);
  }

  .notice[data-kind='error'] {
    background: var(--danger-bg);
    color: var(--danger);
  }

  .text {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    line-height: 1.4;
  }

  .head {
    display: flex;
    gap: var(--space-2);
  }

  .kind {
    font-weight: 600;
  }

  .count {
    opacity: 0.75;
  }

  .message {
    margin: 0;
  }

  details {
    margin-top: var(--space-1);
    opacity: 0.82;
  }

  summary {
    cursor: pointer;
  }

  .detail {
    margin: var(--space-1) 0 0;
    overflow-wrap: anywhere;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
  }

  .dismiss {
    flex-shrink: 0;
    border: none;
    background: none;
    color: inherit;
    cursor: pointer;
    padding: 0 var(--space-1);
    font-size: 16px;
    line-height: 1;
    opacity: 0.7;
  }

  .dismiss:hover {
    opacity: 1;
  }
</style>

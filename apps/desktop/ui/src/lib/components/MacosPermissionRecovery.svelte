<script lang="ts">
  import * as api from '../api'
  import type { PermissionKind } from '../types'

  interface Props {
    kind: PermissionKind
    command: string
    onError: (message: string) => void
  }

  let { kind, command, onError }: Props = $props()
  let copied = $state(false)

  async function openSettings() {
    try {
      await api.openPermissionSettings(kind)
      onError('')
    } catch (error) {
      onError(String(error))
    }
  }

  async function copyCommand() {
    try {
      await navigator.clipboard.writeText(command)
      copied = true
      setTimeout(() => (copied = false), 1500)
    } catch {
      // Best-effort; clipboard access may be denied.
    }
  }
</script>

<pre>{command}</pre>
<div class="actions">
  <button type="button" onclick={openSettings}>Open settings</button>
  <button type="button" onclick={copyCommand}>{copied ? 'Copied' : 'Copy reset command'}</button>
</div>

<style>
  pre {
    padding: var(--space-2);
    border-radius: var(--radius-sm);
    overflow-x: auto;
    background: var(--bg-sunken);
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
</style>

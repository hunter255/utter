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

<div class="actions">
  <button type="button" onclick={openSettings}>Open settings</button>
</div>
<details>
  <summary>Permission recovery</summary>
  <p>Use this only when Utter is missing from System Settings or its saved permission is stale.</p>
  <pre>{command}</pre>
  <button type="button" onclick={copyCommand}>{copied ? 'Copied' : 'Copy reset command'}</button>
</details>

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

  details {
    margin-top: var(--space-2);
    font-size: 12px;
  }

  summary {
    color: var(--text-muted);
    cursor: pointer;
    font-weight: 600;
  }

  p {
    color: var(--text-muted);
  }
</style>

<script lang="ts">
  import { onDestroy, onMount } from 'svelte'
  import { listen, type UnlistenFn } from '@tauri-apps/api/event'
  import { invoke } from '@tauri-apps/api/core'

  import { HUD_STYLE } from './layout'
  import { INITIAL_HUD_STATE, inputSignal, reduceHudState } from './state'
  import { t, type MessageKey } from '../lib/i18n'
  import type { InputSignal } from './state'
  import type { DictationPhase, DictationStatePayload } from '../lib/types'

  const BAR_COUNT = 12
  const barIndices = Array.from({ length: BAR_COUNT }, (_, i) => i)

  const STATE_LABEL: Record<DictationPhase, MessageKey> = {
    idle: 'hud.state.idle',
    recording: 'hud.state.recording',
    transcribing: 'hud.state.transcribing',
    refining: 'hud.state.refining',
    injecting: 'hud.state.injecting',
  }

  const SIGNAL_LABEL: Record<InputSignal, MessageKey> = {
    none: 'hud.signal.none',
    quiet: 'hud.signal.quiet',
    voice: 'hud.signal.voice',
  }

  const EMPTY_PREVIEW: Record<DictationPhase, MessageKey | null> = {
    idle: null,
    recording: 'hud.preview.recording',
    transcribing: 'hud.preview.transcribing',
    refining: 'hud.preview.refining',
    injecting: 'hud.preview.injecting',
  }

  let hud = $state({ ...INITIAL_HUD_STATE })
  let phase = $derived(hud.phase)
  let partial = $derived(hud.partial)
  let signal = $derived(inputSignal(hud.meter))
  let previewText = $derived(
    partial ?? (EMPTY_PREVIEW[phase] ? $t(EMPTY_PREVIEW[phase]) : ''),
  )

  // `hud.meter` is already converted from linear RMS to a perceptual dBFS
  // scale and smoothed in `state.ts`. A non-zero quiet signal always lights
  // at least one bar, so normal speech can no longer look like a dead mic.
  let activeBars = $derived(
    hud.meter <= 0.03 ? 0 : Math.max(1, Math.round(hud.meter * BAR_COUNT)),
  )

  let unlisten: UnlistenFn | undefined

  onMount(() => {
    listen<DictationStatePayload>('dictation-state', (event) => {
      hud = reduceHudState(hud, event.payload)
    }).then((fn) => {
      unlisten = fn
    })
  })

  onDestroy(() => {
    unlisten?.()
  })

  function cancel() {
    // Best-effort: a HUD click that can't reach the runtime (e.g. it never
    // booted) shouldn't throw in the UI.
    invoke('cancel_dictation').catch(() => {})
  }

  function cancelOnKey(event: KeyboardEvent) {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault()
      cancel()
    }
  }
</script>

<div
  class="hud"
  style={HUD_STYLE}
  data-state={phase}
  role="button"
  tabindex="0"
  onclick={cancel}
  onkeydown={cancelOnKey}
>
  <div class="row">
    <span class="status">
      <span class="dot"></span>
      <span class="label">{$t(STATE_LABEL[phase])}</span>
    </span>
    {#if phase === 'recording'}
      <span class="signal" data-signal={signal}>
        <span class="signal-dot"></span>
        {$t(SIGNAL_LABEL[signal])}
      </span>
    {/if}
  </div>
  <div
    class="bars"
    role="meter"
    aria-label={$t('hud.microphoneLevel')}
    aria-valuemin="0"
    aria-valuemax={BAR_COUNT}
    aria-valuenow={activeBars}
  >
    {#each barIndices as i (i)}
      <span class="bar" class:active={i < activeBars}></span>
    {/each}
  </div>
  <div class="partial" class:placeholder={partial === null}><span>{previewText}</span></div>
</div>

<style>
  :global(html),
  :global(body) {
    background: transparent;
  }

  /* The pill sizes itself from its rows (see `./layout.ts`) rather than to a
     pinned height. Its preview row is always reserved while the HUD is
     visible, so neither the pill nor its text jumps when the first partial
     hypothesis arrives.

     Pinning the height here is what hid the preview. A fixed-height flex
     column does not overflow when its rows don't fit — it *shrinks* them,
     and this column's rows fitted only until a third one existed. The
     preview row was squeezed to a few pixels and then clipped away by its
     own `overflow: hidden`: present in the DOM, correct in every event it
     received, and never once drawn. */
  .hud {
    box-sizing: border-box;
    width: 280px;
    padding: var(--hud-pad-y) 14px;
    display: flex;
    flex-direction: column;
    gap: var(--hud-row-gap);
    border-radius: 16px;
    background: rgba(18, 18, 22, 0.86);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
    color: rgba(255, 255, 255, 0.92);
    font-family:
      -apple-system,
      BlinkMacSystemFont,
      'Segoe UI',
      system-ui,
      sans-serif;
    cursor: pointer;
    user-select: none;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    height: var(--hud-status-row);
  }

  .status,
  .signal {
    display: inline-flex;
    align-items: center;
  }

  .status {
    gap: 8px;
    min-width: 0;
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #888;
    flex-shrink: 0;
    transition: background-color 150ms ease;
  }

  .hud[data-state='recording'] .dot {
    background: #ff5c5c;
    box-shadow: 0 0 6px rgba(255, 92, 92, 0.7);
  }

  .hud[data-state='transcribing'] .dot {
    background: #f5a623;
    box-shadow: 0 0 6px rgba(245, 166, 35, 0.7);
  }

  .hud[data-state='refining'] .dot {
    background: #9b59f6;
    box-shadow: 0 0 6px rgba(155, 89, 246, 0.7);
  }

  .hud[data-state='injecting'] .dot {
    background: #2ecc71;
    box-shadow: 0 0 6px rgba(46, 204, 113, 0.7);
  }

  .label {
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-transform: uppercase;
    opacity: 0.85;
  }

  .signal {
    gap: 5px;
    color: rgba(255, 255, 255, 0.64);
    font-size: 10px;
    font-weight: 600;
    white-space: nowrap;
  }

  .signal-dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.3);
  }

  .signal[data-signal='quiet'] .signal-dot {
    background: #f5a623;
    box-shadow: 0 0 5px rgba(245, 166, 35, 0.55);
  }

  .signal[data-signal='voice'] {
    color: rgba(255, 255, 255, 0.86);
  }

  .signal[data-signal='voice'] .signal-dot {
    background: #5ee28a;
    box-shadow: 0 0 5px rgba(94, 226, 138, 0.55);
  }

  .bars {
    display: flex;
    align-items: flex-end;
    gap: 3px;
    height: var(--hud-meter);
    flex-shrink: 0;
  }

  .bar {
    flex: 1;
    height: 20%;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.18);
    transition:
      height 80ms ease,
      background-color 80ms ease;
  }

  .bar:nth-child(3n + 1) {
    height: 35%;
  }
  .bar:nth-child(3n + 2) {
    height: 65%;
  }
  .bar:nth-child(4n) {
    height: 90%;
  }

  .bar.active {
    background: rgba(255, 255, 255, 0.55);
  }

  .hud[data-state='recording'] .bar.active {
    background: #ff5c5c;
  }

  .hud[data-state='transcribing'] .bar.active {
    background: #f5a623;
  }

  .hud[data-state='refining'] .bar.active {
    background: #9b59f6;
  }

  .hud[data-state='injecting'] .bar.active {
    background: #2ecc71;
  }

  /* A live preview only ever grows, so the interesting end is the newest
     one. The row is a fixed two-line box (never taller, never shorter, so a
     window sitting over the user's work never resizes as they speak) with
     its text pinned to the bottom: as the sentence outgrows two lines the
     older lines scroll off the top, whole lines at a time, because the box
     is an exact multiple of the line box. Ellipsising the *end* instead —
     what `text-overflow` does — would have frozen the preview on the first
     few words it ever showed. */
  .partial {
    height: var(--hud-partial);
    display: flex;
    align-items: flex-end;
    overflow: hidden;
    font-size: 12px;
    line-height: var(--hud-partial-line);
    opacity: 0.9;
  }

  .partial span {
    flex: 1;
    overflow-wrap: anywhere;
  }

  .partial.placeholder {
    color: rgba(255, 255, 255, 0.48);
  }
</style>

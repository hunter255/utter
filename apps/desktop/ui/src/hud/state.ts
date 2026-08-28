import type { DictationPhase, DictationStatePayload } from '../lib/types'

/** HUD-only state derived from the backend event stream. */
export interface HudViewState {
  phase: DictationPhase
  /** Perceptual 0..1 meter value, not the backend's linear RMS value. */
  meter: number
  /** Last non-empty streaming hypothesis for the current dictation. */
  partial: string | null
}

export type InputSignal = 'none' | 'quiet' | 'voice'

const METER_FLOOR_DB = -60
const METER_CEILING_DB = -12
const ATTACK = 0.7
const RELEASE = 0.25

export const INITIAL_HUD_STATE: HudViewState = {
  phase: 'idle',
  meter: 0,
  partial: null,
}

/**
 * Maps full-scale linear RMS to a perceptual dBFS meter.
 *
 * Speech commonly occupies only a few hundredths of full scale. A linear
 * meter therefore looks completely dead even though the recognizer receives
 * useful audio. The -60..-12 dBFS window keeps a quiet voice visible without
 * pretending digital silence is signal.
 */
export function rmsToMeter(rms: number): number {
  if (!Number.isFinite(rms) || rms <= 0) return 0

  const clamped = Math.min(1, rms)
  const db = 20 * Math.log10(clamped)
  return Math.min(1, Math.max(0, (db - METER_FLOOR_DB) / (METER_CEILING_DB - METER_FLOOR_DB)))
}

/** Fast attack makes speech visible immediately; slower release prevents jitter. */
export function smoothMeter(previous: number, target: number): number {
  const amount = target > previous ? ATTACK : RELEASE
  return previous + (target - previous) * amount
}

export function inputSignal(meter: number): InputSignal {
  if (meter <= 0.03) return 'none'
  if (meter < 0.38) return 'quiet'
  return 'voice'
}

/**
 * Reduces one backend event without treating `partial: null` as an eraser.
 *
 * Streaming engines use null to mean "the hypothesis did not change on this
 * audio frame". The last hypothesis remains visible through transcription,
 * refinement and injection, then clears at idle or at the start of the next
 * recording.
 */
export function reduceHudState(
  current: HudViewState,
  payload: DictationStatePayload,
): HudViewState {
  const startsRecording = payload.state === 'recording' && current.phase !== 'recording'
  const partialUpdate = payload.partial?.trim() ? payload.partial : null

  let partial = current.partial
  if (payload.state === 'idle') partial = null
  else if (startsRecording) partial = partialUpdate
  else if (partialUpdate !== null) partial = partialUpdate

  const meter =
    payload.state === 'recording'
      ? smoothMeter(startsRecording ? 0 : current.meter, rmsToMeter(payload.level))
      : 0

  return { phase: payload.state, meter, partial }
}

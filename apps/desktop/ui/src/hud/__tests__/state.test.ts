import { describe, expect, it } from 'vitest'

import {
  INITIAL_HUD_STATE,
  inputSignal,
  reduceHudState,
  rmsToMeter,
  smoothMeter,
  type HudViewState,
} from '../state'

describe('HUD input meter', () => {
  it('keeps silence and invalid samples dark', () => {
    expect(rmsToMeter(0)).toBe(0)
    expect(rmsToMeter(Number.NaN)).toBe(0)
    expect(rmsToMeter(-1)).toBe(0)
  })

  it('makes quiet speech visible on a perceptual scale', () => {
    expect(rmsToMeter(0.001)).toBeCloseTo(0)
    expect(rmsToMeter(0.01)).toBeCloseTo(5 / 12)
    expect(rmsToMeter(0.25)).toBeCloseTo(1)
  })

  it('attacks faster than it releases to avoid a jittery display', () => {
    expect(smoothMeter(0, 1)).toBe(0.7)
    expect(smoothMeter(1, 0)).toBe(0.75)
  })

  it('distinguishes no signal, quiet input, and clear voice', () => {
    expect(inputSignal(0)).toBe('none')
    expect(inputSignal(0.2)).toBe('quiet')
    expect(inputSignal(0.7)).toBe('voice')
  })
})

describe('HUD event state', () => {
  const withPreview: HudViewState = {
    phase: 'recording',
    meter: 0.5,
    partial: 'one two',
  }

  it('retains a streaming hypothesis when the next frame has no change', () => {
    const next = reduceHudState(withPreview, {
      state: 'recording',
      level: 0.01,
      partial: null,
    })

    expect(next.partial).toBe('one two')
  })

  it('replaces the hypothesis when the engine emits a real update', () => {
    const next = reduceHudState(withPreview, {
      state: 'recording',
      level: 0.01,
      partial: 'one two three',
    })

    expect(next.partial).toBe('one two three')
  })

  it('keeps the preview visible while the final pipeline runs', () => {
    const transcribing = reduceHudState(withPreview, {
      state: 'transcribing',
      level: 0,
      partial: null,
    })
    const refining = reduceHudState(transcribing, {
      state: 'refining',
      level: 0,
      partial: null,
    })

    expect(transcribing.partial).toBe('one two')
    expect(refining.partial).toBe('one two')
  })

  it('clears the previous dictation at idle and before a new recording', () => {
    const idle = reduceHudState(withPreview, { state: 'idle', level: 0, partial: null })
    expect(idle).toEqual(INITIAL_HUD_STATE)

    const restarted = reduceHudState(
      { ...withPreview, phase: 'injecting' },
      { state: 'recording', level: 0, partial: null },
    )
    expect(restarted.partial).toBeNull()
  })
})

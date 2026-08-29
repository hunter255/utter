import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

const APP = readFileSync(fileURLToPath(new URL('../../App.svelte', import.meta.url)), 'utf8')
const BASE = readFileSync(
  fileURLToPath(new URL('../../styles/base.css', import.meta.url)),
  'utf8',
)

describe('settings window layout contract', () => {
  it('has one vertical scroll owner instead of scrolling body and main together', () => {
    expect(BASE).toMatch(/html,\s*body,\s*#app\s*\{[\s\S]*?overflow:\s*hidden;/)
    expect(APP).toMatch(/\.shell\s*\{[\s\S]*?height:\s*100%;[\s\S]*?overflow:\s*hidden;/)
    expect(APP).toMatch(/nav\s*\{[\s\S]*?overflow-y:\s*hidden;/)
    expect(APP).toMatch(/main\s*\{[\s\S]*?min-height:\s*0;[\s\S]*?overflow-y:\s*auto;/)
  })

  it('lets the content use resized windows and adapts navigation when narrow', () => {
    expect(APP).toMatch(/\.page-content\s*\{[\s\S]*?width:\s*100%;[\s\S]*?max-width:\s*960px;/)
    expect(APP).toContain('@media (max-width: 720px)')
    expect(APP).toContain('@media (max-width: 560px), (max-height: 480px)')
    expect(APP).toMatch(/grid-template-rows:\s*auto minmax\(0, 1fr\);/)
  })

  it('opens a newly selected settings page at its top', () => {
    expect(APP).toContain('{#key hash}')
    expect(APP).not.toContain('bind:this={mainElement}')
  })
})

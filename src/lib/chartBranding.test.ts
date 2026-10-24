import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { BRIDGE_CHART_LABEL, rebrandChartSource } from '../../vite-plugins/chartBranding'

describe('chart branding (The Bridge)', () => {
  it('rewrites the bundled engine default mark', () => {
    const src = readFileSync('node_modules/openalgo-charts/dist/openalgo-charts.mjs', 'utf8')
    expect(src).toContain('Chart by OpenAlgo')
    const out = rebrandChartSource(src)
    expect(out).not.toContain('Chart by OpenAlgo')
    expect(out).toContain(`label:"${BRIDGE_CHART_LABEL}"`)
    expect(out).not.toContain('href:"https://openalgo.in"')
  })
  it('leaves other code alone', () => {
    expect(rebrandChartSource('const a=1')).toBe('const a=1')
  })
})

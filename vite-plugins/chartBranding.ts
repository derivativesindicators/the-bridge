/**
 * The Bridge (AGPL-3.0 modification of OpenAlgo Desktop): the bundled
 * `openalgo-charts` engine stamps a default "Chart by OpenAlgo" mark linking to
 * openalgo.in on every chart. This Vite plugin rewrites that default — once,
 * for every chart and the chart widget — to The Bridge's mark. The
 * "Based on OpenAlgo" credit stays in the app footer and About/licence notices.
 */
import type { Plugin } from 'vite'

export const BRIDGE_CHART_LABEL = 'Chart by The Bridge'
export const BRIDGE_CHART_HREF = 'https://derivativesindicators.com'
export const BRIDGE_CHART_LOGO = '/logo.png'

const UPSTREAM_DEFAULT = 'label:"Chart by OpenAlgo",href:"https://openalgo.in"'

export function rebrandChartSource(code: string): string {
  if (!code.includes(UPSTREAM_DEFAULT)) return code
  return code
    .split(UPSTREAM_DEFAULT)
    .join(
      `src:${JSON.stringify(BRIDGE_CHART_LOGO)},label:${JSON.stringify(BRIDGE_CHART_LABEL)},href:${JSON.stringify(BRIDGE_CHART_HREF)}`
    )
}

const isCharts = (id: string) => /openalgo-charts[\\/]dist[\\/].*\.m?js($|\?)/.test(id)

/** Vite plugin (build + dev pre-bundling). */
export function chartBranding(): Plugin {
  return {
    name: 'the-bridge-chart-branding',
    enforce: 'pre',
    transform(code, id) {
      if (!isCharts(id)) return null
      const out = rebrandChartSource(code)
      return out === code ? null : { code: out, map: null }
    },
  }
}

/** esbuild plugin for `optimizeDeps` so `vite dev` shows the same mark. */
export const chartBrandingEsbuild = {
  name: 'the-bridge-chart-branding',
  // biome-ignore lint/suspicious/noExplicitAny: esbuild types are not a direct dependency
  setup(build: any) {
    build.onLoad({ filter: /openalgo-charts[\\/]dist[\\/].*\.m?js$/ }, async (args: { path: string }) => {
      const fs = await import('node:fs/promises')
      return { contents: rebrandChartSource(await fs.readFile(args.path, 'utf8')), loader: 'js' }
    })
  },
}

import { useEffect, useState } from 'react'

/**
 * The Bridge 1.0.4: why the app's own market data listener is not running
 * (`ws_status` from /settings/api/server), e.g. "port 8770 is already used by
 * another program, most likely OpenAlgo Desktop". Polled only while `active`
 * (the page has lost its live feed), so the banner can say what to fix
 * instead of only "Live feed lost".
 */
export function useFeedServerProblem(active: boolean): string | null {
  const [problem, setProblem] = useState<string | null>(null)
  useEffect(() => {
    if (!active) {
      setProblem(null)
      return
    }
    let cancelled = false
    const check = async () => {
      try {
        const r = await fetch('/settings/api/server', { credentials: 'include' })
        const j = await r.json()
        const ws = j?.data?.ws_status
        if (cancelled) return
        setProblem(ws && ws.state !== 'running' && ws.message ? String(ws.message) : null)
      } catch {
        if (!cancelled) setProblem(null)
      }
    }
    check()
    const id = window.setInterval(check, 15_000)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [active])
  return problem
}

import { useEffect, useState } from 'react'
import { type HostConfig, resolveWebhookBase } from '@/lib/webhookUrl'

/** Base address for webhook URLs: the public tunnel / Host Server URL when set. */
export function useWebhookBase(): { base: string; isPublic: boolean; loaded: boolean } {
  const [host, setHost] = useState<HostConfig | null>(null)
  const [tunnelUrl, setTunnelUrl] = useState<string | null>(null)
  const [loaded, setLoaded] = useState(false)
  useEffect(() => {
    let alive = true
    const getJson = async (path: string) => {
      try {
        const r = await fetch(path, { credentials: 'include' })
        return r.ok ? ((await r.json()) as Record<string, unknown>) : null
      } catch {
        return null
      }
    }
    Promise.all([getJson('/api/config/host'), getJson('/api/tunnel')]).then(([h, t]) => {
      if (!alive) return
      if (h) setHost((h.data as HostConfig | undefined) ?? (h as HostConfig))
      const settings = t?.settings as { enabled?: boolean } | null | undefined
      if (t && settings?.enabled && typeof t.public_url === 'string') setTunnelUrl(t.public_url)
      setLoaded(true)
    })
    return () => {
      alive = false
    }
  }, [])
  return { ...resolveWebhookBase(host, tunnelUrl, window.location.origin), loaded }
}

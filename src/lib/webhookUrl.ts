/**
 * The Bridge 1.0.9: the base address for webhook URLs shown in the app.
 * TradingView (and Chartink) cannot reach http://127.0.0.1:<port>, so when a
 * public address is configured (the Host Server URL, which the ngrok or
 * Cloudflare Tunnel setup sets) webhook URLs use it; otherwise the local
 * address is kept and the UI shows a hint to set up the tunnel.
 */

export interface HostConfig {
  host_server?: string | null
  is_localhost?: boolean
}

export function isLocalAddress(url: string): boolean {
  const low = url.toLowerCase()
  return ['localhost', '127.0.0.1', '0.0.0.0', '[::1]'].some((h) => low.includes(h))
}

/** Pick the public base (host server, then tunnel URL) or fall back to `localOrigin`. */
export function resolveWebhookBase(
  host: HostConfig | null | undefined,
  tunnelPublicUrl: string | null | undefined,
  localOrigin: string
): { base: string; isPublic: boolean } {
  const clean = (u: string) => u.trim().replace(/\/+$/, '')
  const hs = host?.host_server ? clean(host.host_server) : ''
  if (hs && host?.is_localhost !== true && !isLocalAddress(hs)) return { base: hs, isPublic: true }
  const tu = tunnelPublicUrl ? clean(tunnelPublicUrl) : ''
  if (tu && !isLocalAddress(tu)) return { base: tu, isPublic: true }
  return { base: clean(localOrigin), isPublic: false }
}

export function strategyWebhookUrl(base: string, token: string): string {
  return `${base}/strategy/webhook/${token}`
}

export function chartinkWebhookUrl(base: string, webhookId: string): string {
  return `${base}/chartink/webhook/${webhookId}`
}

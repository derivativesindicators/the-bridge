import { CheckCircle2, Globe, Loader2, PlugZap, XCircle } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import { fetchCSRFToken } from '@/api/client'
import { ExternalAnchor } from '@/components/common/ExternalAnchor'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'

/**
 * The Bridge 1.0.5: a guided setup for a webhook URL that never changes, so
 * TradingView alerts reach this computer. Option A: ngrok's free dev domain.
 * Option B: a Cloudflare named tunnel on the trader's own domain. The app
 * downloads the official tunnel program on first use and runs it.
 */

type Provider = 'ngrok' | 'cloudflare'
type RunState =
  | { state: 'off' | 'downloading' | 'starting' | 'running' }
  | { state: 'failed'; message: string }

export interface TunnelStatus {
  configured: boolean
  settings: { provider: Provider; hostname: string; enabled: boolean; webhook_only: boolean } | null
  public_url: string | null
  run: RunState
  http_port: number
}

interface TestResult {
  ok: boolean
  message: string
  ms?: number
}

async function call(path: string, body?: unknown): Promise<Record<string, unknown>> {
  const headers: Record<string, string> = {}
  if (body !== undefined || path !== '/api/tunnel') {
    headers['X-CSRFToken'] = await fetchCSRFToken()
    headers['Content-Type'] = 'application/json'
  }
  const r = await fetch(path, {
    method: body === undefined && path === '/api/tunnel' ? 'GET' : 'POST',
    credentials: 'include',
    headers,
    body: body === undefined ? (path === '/api/tunnel' ? undefined : '{}') : JSON.stringify(body),
  })
  const j = (await r.json().catch(() => ({}))) as Record<string, unknown>
  if (!r.ok) throw new Error(String(j.message ?? `Request failed (${r.status})`))
  return j
}

const RUN_LABEL: Record<string, string> = {
  off: 'Off',
  downloading: 'Downloading tunnel program…',
  starting: 'Starting…',
  running: 'Online',
  failed: 'Problem',
}

export function WebhookTunnelSetup({ onChanged }: { onChanged?: () => void }) {
  const [status, setStatus] = useState<TunnelStatus | null>(null)
  const [provider, setProvider] = useState<Provider>('ngrok')
  const [token, setToken] = useState('')
  const [hostname, setHostname] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [test, setTest] = useState<TestResult | null>(null)
  const [editing, setEditing] = useState(false)

  const refresh = useCallback(async () => {
    try {
      setStatus((await call('/api/tunnel')) as unknown as TunnelStatus)
    } catch {
      /* not signed in yet */
    }
  }, [])

  useEffect(() => {
    refresh()
    const id = window.setInterval(refresh, 5000)
    return () => window.clearInterval(id)
  }, [refresh])

  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true)
    setError(null)
    try {
      await fn()
      await refresh()
      onChanged?.()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const save = () =>
    run(async () => {
      await call('/api/tunnel', { provider, token, hostname })
      setToken('')
      setEditing(false)
    })

  const doTest = () =>
    run(async () => {
      const j = await call('/api/tunnel/test')
      setTest(j.result as TestResult)
    })

  const configured = status?.configured && !editing
  const runState = status?.run?.state ?? 'off'

  return (
    <Card className="mb-8" data-testid="webhook-tunnel-setup">
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Globe className="h-5 w-5" /> Set up a permanent webhook URL
        </CardTitle>
        <CardDescription>
          TradingView sends alerts over the internet, so it needs a public address that reaches this
          computer. Set it up once; the address never changes and The Bridge keeps it running
          whenever the app is open and you are signed in.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        {configured && status?.settings ? (
          <div className="space-y-3">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-mono text-sm">{status.public_url}</span>
              <Badge
                variant={
                  runState === 'running'
                    ? 'default'
                    : runState === 'failed'
                      ? 'destructive'
                      : 'secondary'
                }
              >
                {RUN_LABEL[runState]}
              </Badge>
              <Badge variant="outline">
                {status.settings.provider === 'ngrok' ? 'ngrok' : 'Cloudflare'}
              </Badge>
            </div>
            {status.run.state === 'failed' && (
              <p className="text-sm text-red-600">{(status.run as { message: string }).message}</p>
            )}
            <p className="text-sm text-muted-foreground">
              Only the webhook addresses (/api/v1/…) are reachable through it; every order still
              needs your API key. The Host Server URL is set to this address, so the webhook URL
              below uses it.
            </p>
            <div className="flex flex-wrap gap-2">
              <Button size="sm" onClick={doTest} disabled={busy}>
                <PlugZap className="mr-2 h-4 w-4" /> Test webhook
              </Button>
              {status.settings.enabled ? (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => run(() => call('/api/tunnel/stop'))}
                >
                  Stop
                </Button>
              ) : (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => run(() => call('/api/tunnel/start'))}
                >
                  Start
                </Button>
              )}
              <Button size="sm" variant="outline" disabled={busy} onClick={() => setEditing(true)}>
                Change
              </Button>
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                onClick={() => run(() => call('/api/tunnel/remove'))}
              >
                Remove
              </Button>
            </div>
            {test && (
              <p
                className={`flex items-center gap-2 text-sm ${test.ok ? 'text-green-600' : 'text-red-600'}`}
              >
                {test.ok ? <CheckCircle2 className="h-4 w-4" /> : <XCircle className="h-4 w-4" />}
                {test.message}
                {test.ms != null && ` (${test.ms} ms)`}
              </p>
            )}
          </div>
        ) : (
          <div className="space-y-4">
            <div className="grid gap-2 sm:grid-cols-2">
              <Button
                variant={provider === 'ngrok' ? 'default' : 'outline'}
                onClick={() => setProvider('ngrok')}
              >
                A. ngrok — free, no domain needed (recommended)
              </Button>
              <Button
                variant={provider === 'cloudflare' ? 'default' : 'outline'}
                onClick={() => setProvider('cloudflare')}
              >
                B. Cloudflare Tunnel — your own domain
              </Button>
            </div>
            {provider === 'ngrok' ? (
              <ol className="list-decimal space-y-1 pl-5 text-sm">
                <li>
                  Create a free account at{' '}
                  <ExternalAnchor
                    href="https://dashboard.ngrok.com/signup"
                    className="text-primary underline"
                  >
                    ngrok.com
                  </ExternalAnchor>
                  .
                </li>
                <li>
                  Open{' '}
                  <ExternalAnchor
                    href="https://dashboard.ngrok.com/get-started/your-authtoken"
                    className="text-primary underline"
                  >
                    Your Authtoken
                  </ExternalAnchor>{' '}
                  and copy the token.
                </li>
                <li>
                  Open{' '}
                  <ExternalAnchor
                    href="https://dashboard.ngrok.com/domains"
                    className="text-primary underline"
                  >
                    Domains
                  </ExternalAnchor>{' '}
                  and copy your free dev domain (it ends in .ngrok-free.dev or .ngrok-free.app).
                  Every account gets one; it is yours permanently.
                </li>
                <li>
                  Paste both below and choose Save &amp; start. The Bridge downloads ngrok from
                  ngrok.com the first time.
                </li>
              </ol>
            ) : (
              <ol className="list-decimal space-y-1 pl-5 text-sm">
                <li>Your domain must use Cloudflare for its DNS (free plan is enough).</li>
                <li>
                  In the{' '}
                  <ExternalAnchor
                    href="https://one.dash.cloudflare.com/"
                    className="text-primary underline"
                  >
                    Cloudflare Zero Trust dashboard
                  </ExternalAnchor>
                  , open Networks → Tunnels → Create a tunnel → Cloudflared, and name it (e.g.
                  the-bridge).
                </li>
                <li>
                  On the install step, copy the command shown for any system; it contains the tunnel
                  token. Do not run it.
                </li>
                <li>
                  Add a public hostname (e.g. alerts.yourdomain.com) with service type HTTP and URL{' '}
                  <code>127.0.0.1:{status?.http_port ?? 5050}</code>.
                </li>
                <li>
                  Paste the command (or just the token) and the hostname below. The Bridge downloads
                  cloudflared from Cloudflare's GitHub the first time.
                </li>
              </ol>
            )}
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-1">
                <Label htmlFor="tunnel-token">
                  {provider === 'ngrok' ? 'ngrok authtoken' : 'Tunnel token or install command'}
                </Label>
                <Input
                  id="tunnel-token"
                  type="password"
                  autoComplete="off"
                  value={token}
                  onChange={(e) => setToken(e.target.value)}
                />
              </div>
              <div className="space-y-1">
                <Label htmlFor="tunnel-host">
                  {provider === 'ngrok' ? 'Your dev domain' : 'Public hostname'}
                </Label>
                <Input
                  id="tunnel-host"
                  placeholder={
                    provider === 'ngrok' ? 'abc-123.ngrok-free.dev' : 'alerts.yourdomain.com'
                  }
                  value={hostname}
                  onChange={(e) => setHostname(e.target.value)}
                />
              </div>
            </div>
            <p className="text-xs text-muted-foreground">
              The token is stored encrypted in The Bridge's database and is never shown again or
              written to logs. ngrok's free plan allows 20,000 requests and 1 GB a month — plenty
              for alerts.
            </p>
            <div className="flex gap-2">
              <Button onClick={save} disabled={busy || !token.trim() || !hostname.trim()}>
                {busy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />} Save &amp; start
              </Button>
              {editing && (
                <Button variant="ghost" onClick={() => setEditing(false)}>
                  Cancel
                </Button>
              )}
            </div>
          </div>
        )}
        {error && <p className="text-sm text-red-600">{error}</p>}
      </CardContent>
    </Card>
  )
}

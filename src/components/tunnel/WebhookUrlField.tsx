import { Link } from 'react-router'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useWebhookBase } from '@/hooks/useWebhookBase'
import { strategyWebhookUrl } from '@/lib/webhookUrl'
import { showToast } from '@/utils/toast'

/** Shown under a webhook URL that still points at this computer. */
export function LocalWebhookHint({ service = 'TradingView' }: { service?: string }) {
  return (
    <p className="text-xs text-amber-600" data-testid="webhook-local-hint">
      This is the local address; {service} cannot reach it (TradingView only accepts HTTPS, or HTTP
      on port 80). Set up the public tunnel (ngrok or Cloudflare Tunnel) under{' '}
      <Link to="/tradingview" className="underline">
        TradingView → Webhook tunnel
      </Link>
      ; the webhook URL then uses the tunnel address automatically.
    </p>
  )
}

/** A strategy webhook URL using the public tunnel address when one is set. */
export function StrategyWebhookUrlField({
  token,
  copyable = true,
}: {
  token: string
  copyable?: boolean
}) {
  const { base, isPublic, loaded } = useWebhookBase()
  const url = strategyWebhookUrl(base, token)
  return (
    <div className="space-y-1.5">
      <div className="flex items-center gap-2">
        <Input
          readOnly
          value={url}
          className="font-mono text-xs"
          aria-label="Strategy webhook URL"
        />
        {copyable && (
          <Button
            size="sm"
            onClick={() => {
              navigator.clipboard.writeText(url)
              showToast.success('Copied URL')
            }}
          >
            Copy
          </Button>
        )}
      </div>
      {loaded && !isPublic && <LocalWebhookHint />}
    </div>
  )
}

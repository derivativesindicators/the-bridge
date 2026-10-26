import { describe, expect, it } from 'vitest'
import { chartinkWebhookUrl, resolveWebhookBase, strategyWebhookUrl } from './webhookUrl'

const LOCAL = 'http://127.0.0.1:5001'

describe('resolveWebhookBase', () => {
  it('uses the ngrok Host Server URL', () => {
    const r = resolveWebhookBase(
      { host_server: 'https://abc.ngrok-free.dev/', is_localhost: false },
      null,
      LOCAL
    )
    expect(r).toEqual({ base: 'https://abc.ngrok-free.dev', isPublic: true })
    expect(strategyWebhookUrl(r.base, 'oaws_x')).toBe(
      'https://abc.ngrok-free.dev/strategy/webhook/oaws_x'
    )
  })
  it('uses the Cloudflare tunnel address', () => {
    const r = resolveWebhookBase(
      { host_server: 'https://bridge.example.com', is_localhost: false },
      'https://bridge.example.com',
      LOCAL
    )
    expect(r.base).toBe('https://bridge.example.com')
    expect(chartinkWebhookUrl(r.base, 'w1')).toBe('https://bridge.example.com/chartink/webhook/w1')
  })
  it('falls back to an enabled tunnel URL when the host server is local', () => {
    const r = resolveWebhookBase(
      { host_server: LOCAL, is_localhost: true },
      'https://t.ngrok-free.dev',
      LOCAL
    )
    expect(r).toEqual({ base: 'https://t.ngrok-free.dev', isPublic: true })
  })
  it('keeps the local URL when nothing public is set', () => {
    expect(resolveWebhookBase({ host_server: LOCAL, is_localhost: true }, null, LOCAL)).toEqual({
      base: LOCAL,
      isPublic: false,
    })
    expect(resolveWebhookBase(null, null, `${LOCAL}/`)).toEqual({ base: LOCAL, isPublic: false })
  })
})

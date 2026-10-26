import { render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { StrategyWebhookUrlField } from './WebhookUrlField'

function stub(host: unknown, tunnel: unknown) {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      const body = url.includes('/api/tunnel') ? tunnel : host
      return new Response(JSON.stringify(body), { status: 200 })
    })
  )
}

afterEach(() => vi.unstubAllGlobals())

const view = () =>
  render(
    <MemoryRouter>
      <StrategyWebhookUrlField token="oaws_tok" />
    </MemoryRouter>
  )

describe('StrategyWebhookUrlField', () => {
  it('shows the tunnel address instead of 127.0.0.1', async () => {
    stub(
      { host_server: 'https://abc.ngrok-free.dev', is_localhost: false },
      { settings: { enabled: true }, public_url: 'https://abc.ngrok-free.dev' }
    )
    view()
    const input = screen.getByLabelText('Strategy webhook URL') as HTMLInputElement
    await waitFor(() =>
      expect(input.value).toBe('https://abc.ngrok-free.dev/strategy/webhook/oaws_tok')
    )
    expect(screen.queryByTestId('webhook-local-hint')).toBeNull()
  })
  it('keeps the local URL with a tunnel hint when no tunnel is set', async () => {
    stub({ host_server: 'http://127.0.0.1:5001', is_localhost: true }, { settings: null })
    view()
    await screen.findByTestId('webhook-local-hint')
    const input = screen.getByLabelText('Strategy webhook URL') as HTMLInputElement
    expect(input.value).toBe(`${window.location.origin}/strategy/webhook/oaws_tok`)
    expect(screen.getByRole('link', { name: /Webhook tunnel/ }).getAttribute('href')).toBe(
      '/tradingview'
    )
  })
})

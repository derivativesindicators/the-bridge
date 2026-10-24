import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { WebhookTunnelSetup } from './WebhookTunnelSetup'

const unconfigured = {
  status: 'success',
  configured: false,
  settings: null,
  public_url: null,
  run: { state: 'off' },
  http_port: 5050,
}
const configured = {
  status: 'success',
  configured: true,
  settings: {
    provider: 'ngrok',
    hostname: 'abc.ngrok-free.dev',
    enabled: true,
    webhook_only: true,
  },
  public_url: 'https://abc.ngrok-free.dev',
  run: { state: 'running' },
  http_port: 5050,
}

function stub() {
  let state: unknown = unconfigured
  const posts: Array<{ url: string; body: unknown }> = []
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      const json = (v: unknown) => ({ ok: true, status: 200, json: async () => v })
      if (url === '/auth/csrf-token') return json({ csrf_token: 't' })
      if (init?.method === 'POST') {
        posts.push({ url, body: init.body ? JSON.parse(String(init.body)) : null })
        if (url === '/api/tunnel') state = configured
        if (url === '/api/tunnel/test')
          return json({
            status: 'success',
            result: { ok: true, message: 'Your webhook URL reached The Bridge.', ms: 120 },
          })
        return json(state)
      }
      return json(state)
    })
  )
  return posts
}

afterEach(() => vi.unstubAllGlobals())

describe('WebhookTunnelSetup', () => {
  it('guides ngrok setup, saves token and domain, then tests the webhook', async () => {
    const posts = stub()
    const changed = vi.fn()
    render(<WebhookTunnelSetup onChanged={changed} />)
    expect(await screen.findByText(/Your Authtoken/)).toBeInTheDocument()
    fireEvent.change(screen.getByLabelText('ngrok authtoken'), {
      target: { value: 'tok_1234567890_abcdefghij' },
    })
    fireEvent.change(screen.getByLabelText('Your dev domain'), {
      target: { value: 'abc.ngrok-free.dev' },
    })
    fireEvent.click(screen.getByRole('button', { name: /Save & start/ }))
    await waitFor(() => expect(screen.getByText('https://abc.ngrok-free.dev')).toBeInTheDocument())
    expect(posts[0]).toEqual({
      url: '/api/tunnel',
      body: {
        provider: 'ngrok',
        token: 'tok_1234567890_abcdefghij',
        hostname: 'abc.ngrok-free.dev',
      },
    })
    expect(changed).toHaveBeenCalled()
    expect(screen.getByText('Online')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /Test webhook/ }))
    expect(await screen.findByText(/reached The Bridge/)).toBeInTheDocument()
  })

  it('shows Cloudflare steps with the app port', async () => {
    stub()
    render(<WebhookTunnelSetup />)
    fireEvent.click(await screen.findByRole('button', { name: /Cloudflare Tunnel/ }))
    expect(screen.getByText('127.0.0.1:5050')).toBeInTheDocument()
    expect(screen.getByLabelText('Public hostname')).toBeInTheDocument()
  })
})

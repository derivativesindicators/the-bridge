import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { resetWindowProofForTests } from '@/lib/desktop'
import { fireEvent, render, screen, waitFor } from '@/test/test-utils'
import Login from './Login'

const shell = vi.hoisted(() => ({ inShell: false }))

vi.mock('@tauri-apps/api/core', async (original) => ({
  ...(await original<typeof import('@tauri-apps/api/core')>()),
  isTauri: () => shell.inShell,
  invoke: async (cmd: string) => (cmd === 'window_proof' ? 'proof-from-the-shell' : null),
}))

// Desktop: one sign-in card. After the password the server says where to
// go: the dashboard when the saved broker session was resumed, otherwise
// Select Broker. No page in between.

function serve(opts: {
  needsSetup?: boolean
  login: Record<string, unknown>
  totp?: unknown
  /** Answers to successive sign-ins (with their status), before `login`. */
  logins?: { status: number; body: Record<string, unknown> }[]
}) {
  const queue = [...(opts.logins ?? [])]
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status })
      if (url === '/auth/check-setup') return json({ needs_setup: opts.needsSetup ?? false })
      if (url === '/auth/session-status') {
        return json({ status: 'success', authenticated: false, logged_in: false })
      }
      if (url === '/auth/csrf-token') return json({ csrf_token: 'tok' })
      if (url === '/auth/login') {
        const next = queue.shift()
        return new Response(JSON.stringify(next ? next.body : opts.login), {
          status: next ? next.status : 200,
          headers: { 'content-type': 'application/json' },
        })
      }
      if (url === '/auth/login/totp') return json(opts.totp ?? { status: 'success' })
      return json({}, 404)
    })
  )
}

async function signIn() {
  fireEvent.change(await screen.findByLabelText('Username'), { target: { value: 'trader' } })
  fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'Secret@123' } })
  fireEvent.click(screen.getByRole('button', { name: /sign in/i }))
}

beforeEach(() => {
  window.history.pushState({}, '', '/login')
})

afterEach(() => {
  vi.unstubAllGlobals()
  window.history.pushState({}, '', '/')
  shell.inShell = false
  resetWindowProofForTests()
})

describe('Sign-in routing', () => {
  it('goes straight to the dashboard when the broker session was resumed', async () => {
    serve({
      login: { status: 'success', redirect: '/dashboard', broker: 'zerodha' },
    })
    render(<Login />)
    await signIn()
    await waitFor(() => expect(window.location.pathname).toBe('/dashboard'))
  })

  it('goes to Select Broker when no broker session was resumed', async () => {
    serve({ login: { status: 'success', redirect: '/broker' } })
    render(<Login />)
    await signIn()
    await waitFor(() => expect(window.location.pathname).toBe('/broker'))
  })

  it('goes to Select Broker when the server names no page', async () => {
    serve({ login: { status: 'success' } })
    render(<Login />)
    await signIn()
    await waitFor(() => expect(window.location.pathname).toBe('/broker'))
  })

  it('asks for the authenticator code first when two-factor sign-in is on', async () => {
    serve({
      login: { status: 'totp_required', message: 'Enter the 6-digit code.' },
      totp: { status: 'success', redirect: '/broker' },
    })
    render(<Login />)
    await signIn()
    const code = await screen.findByLabelText('Authentication code')
    expect(window.location.pathname).toBe('/login')
    fireEvent.change(code, { target: { value: '123456' } })
    fireEvent.click(screen.getByRole('button', { name: 'Verify code' }))
    await waitFor(() => expect(window.location.pathname).toBe('/broker'))
  })

  it("shows the server's reason and stays on the sign-in card", async () => {
    serve({ login: { status: 'error', message: 'Invalid credentials' } })
    render(<Login />)
    await signIn()
    expect(await screen.findByText('Invalid credentials')).toBeInTheDocument()
    expect(window.location.pathname).toBe('/login')
  })
})

describe('Sign-in card', () => {
  it('offers account setup only when no account exists yet', async () => {
    serve({ needsSetup: true, login: { status: 'success' } })
    const { unmount } = render(<Login />)
    expect(await screen.findByRole('link', { name: 'Set up your account' })).toHaveAttribute(
      'href',
      '/setup'
    )
    unmount()
    vi.unstubAllGlobals()
    serve({ needsSetup: false, login: { status: 'success' } })
    render(<Login />)
    await screen.findByLabelText('Username')
    expect(screen.queryByRole('link', { name: 'Set up your account' })).toBeNull()
    // The recovery path (password reset, then the reset-account card).
    expect(screen.getByRole('link', { name: 'Forgot password?' })).toHaveAttribute(
      'href',
      '/reset-password'
    )
  })
})

// KEY-01: the first sign-in with this build moves an earlier build's keys
// out of the system keychain once. When the keychain cannot be read the
// server offers two ways out.
describe('Keys of an earlier build', () => {
  const unreadable = {
    status: 503,
    body: {
      status: 'error',
      code: 'keychain_unreadable',
      message: 'The Bridge needs to read your keys from the system keychain one last time.',
      recovery: ['retry_keychain', 'start_fresh'],
    },
  }

  it('retries the keychain with what was typed, then signs in', async () => {
    serve({ logins: [unreadable], login: { status: 'success', redirect: '/broker' } })
    render(<Login />)
    await signIn()
    expect(await screen.findByText(/one last time/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Allow the keychain once and retry' }))
    await waitFor(() => expect(window.location.pathname).toBe('/broker'))
    const calls = (fetch as unknown as { mock: { calls: unknown[][] } }).mock.calls
    expect(calls.filter((c) => c[0] === '/auth/login')).toHaveLength(2)
  })

  it('offers starting fresh, which opens the new-password form', async () => {
    serve({ logins: [unreadable], login: { status: 'success' } })
    render(<Login />)
    await signIn()
    fireEvent.click(await screen.findByRole('button', { name: 'Start fresh' }))
    expect(await screen.findByRole('heading', { name: 'Start fresh' })).toBeInTheDocument()
    expect(screen.getByText(/enter your broker keys again/)).toBeInTheDocument()
  })

  it('offers only a fresh start when the keychain has no keys for this account', async () => {
    serve({
      logins: [
        {
          status: 409,
          body: {
            status: 'error',
            code: 'keys_missing',
            message: "The keys for this account are not in this computer's keychain.",
            recovery: ['start_fresh'],
          },
        },
      ],
      login: { status: 'success' },
    })
    render(<Login />)
    await signIn()
    expect(await screen.findByText(/not in this computer's keychain/)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Allow the keychain once and retry' })).toBeNull()
    expect(screen.getByRole('button', { name: 'Start fresh' })).toBeInTheDocument()
  })

  it('tells the trader what stays off until they sign in', async () => {
    serve({ login: { status: 'success' } })
    render(<Login />)
    await screen.findByLabelText('Username')
    expect(
      screen.getByText(/Until you sign in, the API, market data, webhooks, AI clients/)
    ).toBeInTheDocument()
  })
})

// KEY-01 security review: only the desktop window's sign-in carries the
// proof that lets the server move an earlier build's keys and say why keys
// could not be opened; a browser tab's sign-in carries nothing.
describe('The app window proof', () => {
  const loginHeaders = () => {
    const calls = (fetch as unknown as { mock: { calls: [string, RequestInit?][] } }).mock.calls
    const call = calls.find((c) => c[0] === '/auth/login')
    return new Headers(call?.[1]?.headers)
  }

  it('goes with the password from the desktop window', async () => {
    shell.inShell = true
    serve({ login: { status: 'success', redirect: '/broker' } })
    render(<Login />)
    await signIn()
    await waitFor(() => expect(window.location.pathname).toBe('/broker'))
    expect(loginHeaders().get('X-OpenAlgo-Window')).toBe('proof-from-the-shell')
  })

  it('is not sent from a browser tab', async () => {
    serve({ login: { status: 'success', redirect: '/broker' } })
    render(<Login />)
    await signIn()
    await waitFor(() => expect(window.location.pathname).toBe('/broker'))
    expect(loginHeaders().has('X-OpenAlgo-Window')).toBe(false)
  })
})

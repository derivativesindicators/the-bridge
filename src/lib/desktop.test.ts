import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const tauri = vi.hoisted(() => ({
  inShell: false,
  open: vi.fn(async (_url: string) => {}),
  invoke: vi.fn(async (_cmd: string): Promise<unknown> => 'proof-from-the-shell'),
}))

vi.mock('@tauri-apps/api/core', () => ({ isTauri: () => tauri.inShell, invoke: tauri.invoke }))
vi.mock('@tauri-apps/plugin-shell', () => ({ open: tauri.open }))

import {
  appWindowHeaders,
  brokerSignInTarget,
  DEFAULT_WEBSOCKET_URL,
  desktopBrokerLoginUrl,
  desktopProfileMenuItems,
  installDesktopShellHandlers,
  isDesktopShell,
  isExternalHttpUrl,
  layoutRedirect,
  resetWindowProofForTests,
  signInWithSavedKeys,
  WINDOW_PROOF_HEADER,
} from './desktop'

const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

function clickLink(href: string, attrs: Record<string, string> = {}): MouseEvent {
  const link = document.createElement('a')
  link.setAttribute('href', href)
  for (const [k, v] of Object.entries(attrs)) link.setAttribute(k, v)
  link.textContent = 'link'
  document.body.appendChild(link)
  const event = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 })
  link.dispatchEvent(event)
  link.remove()
  return event
}

describe('isExternalHttpUrl', () => {
  const base = 'http://127.0.0.1:5000/dashboard'

  it('treats another origin over http(s) as external', () => {
    expect(isExternalHttpUrl('https://docs.openalgo.in', base)).toBe(true)
    expect(isExternalHttpUrl('http://127.0.0.1:5001/x', base)).toBe(true)
  })

  it('keeps the app origin, relative paths and non-web schemes inside the app', () => {
    expect(isExternalHttpUrl('/orderbook', base)).toBe(false)
    expect(isExternalHttpUrl('http://127.0.0.1:5000/positions', base)).toBe(false)
    expect(isExternalHttpUrl('mailto:support@openalgo.in', base)).toBe(false)
    expect(isExternalHttpUrl('blob:http://127.0.0.1:5000/abc', base)).toBe(false)
    expect(isExternalHttpUrl('javascript:void(0)', base)).toBe(false)
  })
})

describe('desktopBrokerLoginUrl', () => {
  it('sends every broker the server calls a redirect to the server-side sign-in start', () => {
    // Shoonya, Zebu, TradeSmart and RMoney were missing from a list the page
    // used to keep, so their Connect went to /<broker>/callback and failed.
    for (const b of ['zerodha', 'dhan', 'aliceblue', 'shoonya', 'zebu', 'tradesmart', 'rmoney']) {
      expect(desktopBrokerLoginUrl(b, 'redirect')).toBe(`/${b}/initiate-oauth`)
    }
  })

  it('leaves form brokers, and brokers the server did not describe, to the web flow', () => {
    expect(desktopBrokerLoginUrl('angel', 'form')).toBeNull()
    expect(desktopBrokerLoginUrl('fivepaisaxts', 'saved_keys')).toBeNull()
    expect(desktopBrokerLoginUrl('zerodha', undefined)).toBeNull()
    expect(desktopBrokerLoginUrl('', 'redirect')).toBeNull()
  })
})

describe('signInWithSavedKeys', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('posts to the broker callback with the CSRF token and no login fields', async () => {
    const calls: Array<[string, RequestInit | undefined]> = []
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string, init?: RequestInit) => {
        calls.push([url, init])
        if (url === '/auth/csrf-token') return new Response(JSON.stringify({ csrf_token: 't1' }))
        return new Response(JSON.stringify({ status: 'success', redirect: '/dashboard' }))
      })
    )
    await expect(signInWithSavedKeys('jainamxts')).resolves.toBeNull()
    const [url, init] = calls[1]
    expect(url).toBe('/jainamxts/callback')
    expect(init?.method).toBe('POST')
    const headers = init?.headers as Record<string, string> | undefined
    const body = init?.body as FormData | undefined
    expect(headers?.['X-CSRFToken']).toBe('t1')
    expect([...(body?.entries() ?? [])]).toEqual([['csrf_token', 't1']])
  })

  it("returns the server's message when the broker refuses", async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) =>
        url === '/auth/csrf-token'
          ? new Response(JSON.stringify({ csrf_token: 't1' }))
          : new Response(JSON.stringify({ status: 'error', message: 'Check your API key.' }), {
              status: 401,
            })
      )
    )
    await expect(signInWithSavedKeys('deltaexchange')).resolves.toBe('Check your API key.')
  })

  it('says The Bridge could not be reached when the request fails', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string) => {
        if (url === '/auth/csrf-token') return new Response(JSON.stringify({ csrf_token: 't1' }))
        throw new TypeError('Failed to fetch')
      })
    )
    await expect(signInWithSavedKeys('wisdom')).resolves.toBe(
      'Could not reach The Bridge. Try again.'
    )
  })
})

describe('desktop constants', () => {
  it('falls back to the development feed port under vitest', () => {
    expect(DEFAULT_WEBSOCKET_URL).toBe('ws://127.0.0.1:8766')
  })

  it('adds Server Settings to the profile menu', () => {
    expect(desktopProfileMenuItems.map((i) => i.href)).toEqual(['/settings/server'])
  })
})

describe('installDesktopShellHandlers', () => {
  let uninstall: () => void = () => {}
  const originalOpen = window.open

  beforeEach(() => {
    tauri.open.mockClear()
  })

  afterEach(() => {
    uninstall()
    tauri.inShell = false
    window.open = originalOpen
  })

  it('does nothing in a plain browser', () => {
    tauri.inShell = false
    expect(isDesktopShell()).toBe(false)
    uninstall = installDesktopShellHandlers()
    expect(window.open).toBe(originalOpen)
    const event = clickLink('https://docs.openalgo.in', { target: '_blank' })
    expect(event.defaultPrevented).toBe(false)
    expect(tauri.open).not.toHaveBeenCalled()
  })

  it('sends external links to the system browser inside the shell', async () => {
    tauri.inShell = true
    uninstall = installDesktopShellHandlers()
    const event = clickLink('https://docs.openalgo.in/', { target: '_blank' })
    await flush()
    expect(event.defaultPrevented).toBe(true)
    expect(tauri.open).toHaveBeenCalledWith('https://docs.openalgo.in/')
  })

  it('leaves in-app links to the router', async () => {
    tauri.inShell = true
    uninstall = installDesktopShellHandlers()
    const event = clickLink('/orderbook')
    await flush()
    expect(event.defaultPrevented).toBe(false)
    expect(tauri.open).not.toHaveBeenCalled()
  })

  it('respects a click a component already handled', async () => {
    tauri.inShell = true
    uninstall = installDesktopShellHandlers()
    const link = document.createElement('a')
    link.href = 'https://docs.openalgo.in/'
    link.addEventListener('click', (e) => e.preventDefault())
    document.body.appendChild(link)
    link.click()
    link.remove()
    await flush()
    expect(tauri.open).not.toHaveBeenCalled()
  })

  it('routes window.open: external to the browser, same-origin exports to a download', async () => {
    tauri.inShell = true
    uninstall = installDesktopShellHandlers()
    expect(window.open('https://openalgo.in/discord', '_blank')).toBeNull()
    await flush()
    expect(tauri.open).toHaveBeenCalledWith('https://openalgo.in/discord')

    const clicks: string[] = []
    const spy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
      this: HTMLAnchorElement
    ) {
      clicks.push(this.getAttribute('href') ?? '')
    })
    expect(window.open('/traffic/export', '_blank')).toBeNull()
    spy.mockRestore()
    expect(clicks).toEqual(['/traffic/export'])
    expect(tauri.open).toHaveBeenCalledTimes(1)
  })

  it('restores window.open when removed', () => {
    tauri.inShell = true
    uninstall = installDesktopShellHandlers()
    expect(window.open).not.toBe(originalOpen)
    uninstall()
    uninstall = () => {}
    expect(window.open).toBe(originalOpen)
  })
})

describe('isPreBrokerPath', () => {
  it('lets Profile and Server Settings open before a broker is connected', async () => {
    const { isPreBrokerPath } = await import('./desktop')
    expect(isPreBrokerPath('/profile')).toBe(true)
    expect(isPreBrokerPath('/profile/')).toBe(true)
    expect(isPreBrokerPath('/settings/server')).toBe(true)
  })

  it('keeps every trading page behind the broker login', async () => {
    const { isPreBrokerPath } = await import('./desktop')
    for (const p of ['/', '/dashboard', '/orderbook', '/positions', '/profiles', '/apikey']) {
      expect(isPreBrokerPath(p)).toBe(false)
    }
  })
})

describe('brokerSignInTarget', () => {
  it('opens each sign-in the way the server says the broker signs in', () => {
    expect(brokerSignInTarget('zerodha', 'redirect')).toBe('/zerodha/initiate-oauth')
    expect(brokerSignInTarget('angel', 'form')).toBe('/angel/callback')
    // A saved-keys broker signs in with one action, not a page.
    expect(brokerSignInTarget('fivepaisaxts', 'saved_keys')).toBeNull()
    expect(brokerSignInTarget('zerodha', undefined)).toBeNull()
    expect(brokerSignInTarget('', 'form')).toBeNull()
  })
})

// One rule for every guard: not signed in goes to /login; signed in with no
// broker logged in goes straight to /broker (no detour through /login).
describe('layoutRedirect', () => {
  it('sends a trader who is not signed in to the sign-in page', () => {
    expect(layoutRedirect(null, false, '/dashboard')).toBe('/login')
    expect(layoutRedirect({ username: '', broker: null }, false, '/profile')).toBe('/login')
  })

  it('sends a signed-in trader with no broker logged in to Select Broker', () => {
    // As AuthSync stores it after a reload, and as Login stores it.
    expect(layoutRedirect({ username: 'trader', broker: null }, false, '/dashboard')).toBe(
      '/broker'
    )
    expect(layoutRedirect({ username: 'trader', broker: '' }, true, '/positions')).toBe('/broker')
    // Profile and Server Settings stay open before a broker login.
    expect(layoutRedirect({ username: 'trader', broker: null }, false, '/profile')).toBeNull()
    expect(
      layoutRedirect({ username: 'trader', broker: null }, false, '/settings/server')
    ).toBeNull()
  })

  it('lets a trader with a broker logged in through', () => {
    expect(layoutRedirect({ username: 'trader', broker: 'zerodha' }, true, '/dashboard')).toBeNull()
  })
})

// KEY-01 security review: the server treats a sign-in as the app window's
// only when it carries the proof the desktop shell hands to that window.
describe('appWindowHeaders', () => {
  beforeEach(() => {
    resetWindowProofForTests()
    tauri.invoke.mockClear()
  })
  afterEach(() => {
    tauri.inShell = false
    tauri.invoke.mockImplementation(async () => 'proof-from-the-shell')
  })

  it('sends nothing from a browser tab, and never asks the shell', async () => {
    tauri.inShell = false
    expect(await appWindowHeaders()).toEqual({})
    expect(tauri.invoke).not.toHaveBeenCalled()
  })

  it('sends the proof from the desktop window, asking the shell once', async () => {
    tauri.inShell = true
    expect(await appWindowHeaders()).toEqual({ [WINDOW_PROOF_HEADER]: 'proof-from-the-shell' })
    expect(await appWindowHeaders()).toEqual({ [WINDOW_PROOF_HEADER]: 'proof-from-the-shell' })
    expect(tauri.invoke).toHaveBeenCalledTimes(1)
    expect(tauri.invoke).toHaveBeenCalledWith('window_proof')
  })

  it('sends nothing when the shell refuses, and asks again next time', async () => {
    tauri.inShell = true
    tauri.invoke.mockImplementationOnce(async () => {
      throw new Error('refused')
    })
    expect(await appWindowHeaders()).toEqual({})
    expect(await appWindowHeaders()).toEqual({ [WINDOW_PROOF_HEADER]: 'proof-from-the-shell' })
    expect(tauri.invoke).toHaveBeenCalledTimes(2)
  })
})

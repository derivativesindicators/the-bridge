/**
 * Everything the desktop app does differently from OpenAlgo web, in one place.
 *
 * The frontend is the web frontend carried over nearly verbatim, so that a web
 * change can be applied here by diff. The few things a desktop shell needs on
 * top live in this module, and each web file that uses it carries a single
 * "Desktop:" comment explaining why. Keep it small: business logic belongs in
 * the local server, which the app reaches over HTTP and Socket.IO exactly as
 * the web frontend reaches Flask.
 *
 * The same page can also be opened in an ordinary browser pointed at the local
 * server, so every helper here works outside the Tauri shell too.
 */

import { invoke, isTauri } from '@tauri-apps/api/core'
import { Server } from 'lucide-react'
import type { UpdateActivity, UpdateWarning } from '@/api/updates'
import type { NavItem } from '@/config/navigation'

/** True when running inside the Tauri window rather than a plain browser tab. */
export function isDesktopShell(): boolean {
  try {
    return isTauri()
  } catch {
    return false
  }
}

/**
 * The market data feed address used when the server does not report one.
 *
 * Matches the shipped default (8765) and the development port (8766) the
 * maintainer uses so the desktop can run next to OpenAlgo web on one machine.
 */
export const DEFAULT_WEBSOCKET_URL = import.meta.env.DEV
  ? 'ws://127.0.0.1:8766'
  : 'ws://127.0.0.1:8765'

/**
 * How a broker's sign-in starts, as the local server reports it (`sign_in`
 * on GET /api/broker/list, /api/broker/configured and /auth/broker-config).
 * The pages keep no broker list of their own, so they cannot drift from the
 * server's broker catalogue.
 *
 * - `redirect`: the broker's own login page. The web builds that address in
 *   the browser from the broker API key; the desktop never sends the key to
 *   the page (/auth/broker-config returns broker_api_key as null), so the
 *   local server builds the address, records the sign-in and redirects:
 *   GET /<broker>/initiate-oauth.
 * - `form`: the broker's in-app page, through GET /<broker>/callback as on
 *   the web.
 * - `saved_keys`: the saved keys are all the broker needs (the XTS brokers,
 *   Delta Exchange, Dhan Sandbox), so the page signs in with one action, as
 *   the web does on its first visit to /<broker>/callback.
 */
export type BrokerSignIn = 'redirect' | 'form' | 'saved_keys'

/** Where to send the browser to sign in to a redirect broker, or null. */
export function desktopBrokerLoginUrl(
  broker: string,
  signIn: BrokerSignIn | undefined
): string | null {
  if (!broker || signIn !== 'redirect') return null
  return `/${encodeURIComponent(broker)}/initiate-oauth`
}

/**
 * The page a broker's "Login with" opens, or null for a saved-keys broker,
 * which signs in with one action instead (`signInWithSavedKeys`). A form
 * broker opens through its callback address, as on the web, so the server
 * can send Definedge's and Nubra's login OTP and route Samco to its connect
 * page before the form shows.
 */
export function brokerSignInTarget(
  broker: string,
  signIn: BrokerSignIn | undefined
): string | null {
  if (!broker) return null
  if (signIn === 'redirect') return desktopBrokerLoginUrl(broker, signIn)
  if (signIn === 'form') return `/${encodeURIComponent(broker)}/callback`
  return null
}

/**
 * Sign in to a broker whose saved keys are all it needs: a POST to its
 * callback carrying the CSRF token (as the broker login form posts it) and
 * no login fields. Returns null when signed in, else the server's message
 * for the trader.
 */
export async function signInWithSavedKeys(broker: string): Promise<string | null> {
  const csrf = await csrfToken()
  if (!csrf) return 'Could not reach The Bridge. Try again.'
  const form = new FormData()
  form.append('csrf_token', csrf)
  try {
    const res = await fetch(`/${encodeURIComponent(broker)}/callback`, {
      method: 'POST',
      credentials: 'include',
      headers: { 'X-CSRFToken': csrf },
      body: form,
    })
    const body = await res.json().catch(() => null)
    if (res.ok && body?.status === 'success') return null
    return body?.message || 'Could not sign in to your broker. Try again.'
  } catch {
    return 'Could not reach The Bridge. Try again.'
  }
}

/** Profile menu entries that exist only in the desktop app. */
export const desktopProfileMenuItems: NavItem[] = [
  { href: '/settings/server', label: 'Server Settings', icon: Server },
]

/** An absolute http(s) address on a different origin than the app itself. */
export function isExternalHttpUrl(href: string, base: string = window.location.href): boolean {
  let url: URL
  try {
    url = new URL(href, base)
  } catch {
    return false
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return false
  return url.origin !== new URL(base).origin
}

/**
 * Open a web page in the user's default browser.
 *
 * Inside the Tauri window a target="_blank" link or window.open() has no
 * browser tab to land in, so the shell plugin hands the address to the OS.
 * In a plain browser this is an ordinary new tab.
 */
export async function openExternal(url: string): Promise<void> {
  if (isDesktopShell()) {
    const { open } = await import('@tauri-apps/plugin-shell')
    await open(url)
    return
  }
  window.open(url, '_blank', 'noopener,noreferrer')
}

/**
 * Save a same-origin download (an export endpoint) without opening a window.
 *
 * The web opens exports with window.open(url, '_blank') and lets the new tab
 * turn into a download. The desktop window cannot open tabs, so the same
 * request is made through a temporary download link instead; the server's
 * Content-Disposition header still names the file.
 */
function downloadInPlace(url: string): void {
  const link = document.createElement('a')
  link.href = url
  link.download = ''
  link.rel = 'noopener'
  link.style.display = 'none'
  document.body.appendChild(link)
  link.click()
  link.remove()
}

function anchorFromEvent(event: MouseEvent): HTMLAnchorElement | null {
  const target = event.target
  if (!(target instanceof Element)) return null
  return target.closest('a[href]') as HTMLAnchorElement | null
}

/**
 * Route external links and new-window requests through the desktop shell.
 *
 * Installed once at startup. Does nothing in a plain browser. Returns a
 * function that removes the handlers again (used by tests).
 */
export function installDesktopShellHandlers(): () => void {
  if (!isDesktopShell()) return () => {}

  // Bubble phase on window: React's own onClick handlers (and any
  // preventDefault they call) have already run by the time this fires.
  const onClick = (event: MouseEvent) => {
    if (event.defaultPrevented || event.button !== 0) return
    const anchor = anchorFromEvent(event)
    if (!anchor) return
    const href = anchor.getAttribute('href') ?? ''
    if (!isExternalHttpUrl(href)) return
    event.preventDefault()
    void openExternal(anchor.href)
  }
  window.addEventListener('click', onClick)

  const originalOpen = window.open
  window.open = (url?: string | URL, target?: string, features?: string) => {
    const href = url === undefined ? '' : String(url)
    if (href && isExternalHttpUrl(href)) {
      void openExternal(new URL(href, window.location.href).toString())
      return null
    }
    if (href && (target === '_blank' || target === undefined)) {
      downloadInPlace(href)
      return null
    }
    return originalOpen.call(window, url, target, features)
  }

  return () => {
    window.removeEventListener('click', onClick)
    window.open = originalOpen
  }
}

/**
 * Pages inside the app layout that a signed-in trader can open before
 * logging in to a broker: Profile, and Server Settings, which may be needed
 * to free a port first. The broker pages themselves (Select Broker, Broker
 * Configuration, the broker login forms) sit outside the layout.
 */
export const DESKTOP_PRE_BROKER_PATHS = ['/profile', '/settings/server'] as const

export function isPreBrokerPath(pathname: string): boolean {
  const p = pathname.replace(/\/+$/, '') || '/'
  return (DESKTOP_PRE_BROKER_PATHS as readonly string[]).includes(p)
}

/**
 * Where a page inside the app layout sends a trader who may not reach it,
 * or null when they may. One rule for every guard, so no page takes a
 * detour through another:
 *
 * - not signed in to OpenAlgo: the sign-in page;
 * - signed in with no broker logged in: Select Broker (except the pages
 *   in `DESKTOP_PRE_BROKER_PATHS`);
 * - otherwise the page itself.
 */
export function layoutRedirect(
  user: { username?: string | null; broker?: string | null } | null | undefined,
  isAuthenticated: boolean,
  pathname: string
): string | null {
  if (user?.broker && isAuthenticated) return null
  if (!user?.username) return '/login'
  if (!user.broker) return isPreBrokerPath(pathname) ? null : '/broker'
  return '/login'
}

/**
 * The Broker Configuration page, where each broker's keys are saved. The
 * web has no such page (it reads one broker's keys from `.env`).
 */
export const DESKTOP_BROKER_SETUP_PATH = '/broker/config'

/** The Broker Configuration page with one broker selected. */
export function brokerConfigPath(broker?: string): string {
  return broker
    ? `${DESKTOP_BROKER_SETUP_PATH}?broker=${encodeURIComponent(broker)}`
    : DESKTOP_BROKER_SETUP_PATH
}

/** Initial Profile tab from `?tab=`, so the broker page can link to it. */
export function desktopInitialProfileTab(fallback: string): string {
  try {
    return new URLSearchParams(window.location.search).get('tab') || fallback
  } catch {
    return fallback
  }
}

/**
 * The broker whose session is live in OpenAlgo, from a `/auth/session-status`
 * answer, or null when none is.
 */
export function liveBrokerOf(status: unknown): string | null {
  const s = status as { logged_in?: unknown; broker?: unknown } | null
  return s?.logged_in === true && typeof s.broker === 'string' && s.broker ? s.broker : null
}

/**
 * The broker whose session is live now. A switch to another broker ends it,
 * so the pages confirm first. When the server cannot be asked, `fallback`
 * (the active broker) is assumed live, so the trader is still asked.
 */
export async function fetchLiveBroker(fallback: string | null): Promise<string | null> {
  try {
    const res = await fetch('/auth/session-status', { credentials: 'include' })
    if (!res.ok) return fallback
    return liveBrokerOf(await res.json())
  } catch {
    return fallback
  }
}

// ============================================================================
// MCP access tokens for AI clients (API key page; desktop only)
// ============================================================================

/** An MCP token as listed by GET /api/mcp/tokens (never the token itself). */
export interface McpToken {
  id: number
  name: string
  scope: 'read' | 'read_write'
  token_prefix: string
  created_at: string
  last_used_at: string | null
}

/** Ready-to-paste client configuration from the local server. */
export interface McpClientConfig {
  executable: string
  server_url: string
  mcp_url: string
  claude_desktop: unknown
  /** Carries `<MCP_TOKEN>`, never the token: a shell keeps a history. */
  claude_code: string
  claude_code_note?: string
}

async function csrfToken(): Promise<string | undefined> {
  return fetch('/auth/csrf-token', { credentials: 'include' })
    .then((r) => r.json())
    .then((b) => b?.csrf_token as string | undefined)
    .catch(() => undefined)
}

async function mcpWrite(url: string, method: 'POST' | 'DELETE', body?: unknown) {
  const csrf = await csrfToken()
  if (!csrf) throw new Error('Could not reach The Bridge. Try again.')
  const res = await fetch(url, {
    method,
    credentials: 'include',
    headers: { 'Content-Type': 'application/json', 'X-CSRFToken': csrf },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const data = await res.json().catch(() => null)
  if (!res.ok) throw new Error(data?.message || 'The request failed. Try again.')
  return data
}

/** Live MCP tokens, newest first. */
export async function fetchMcpTokens(): Promise<McpToken[]> {
  const res = await fetch('/api/mcp/tokens', { credentials: 'include' })
  if (!res.ok) return []
  const body = await res.json().catch(() => null)
  return Array.isArray(body?.data) ? body.data : []
}

/** Create a token; the token and its client configuration come back once. */
export async function createMcpToken(
  name: string,
  scope: 'read' | 'read_write'
): Promise<{ token: string; client_config: McpClientConfig }> {
  const data = await mcpWrite('/api/mcp/tokens', 'POST', { name, scope })
  return { token: data.token, client_config: data.client_config }
}

/** Revoke a token: clients using it stop working at once. */
export async function revokeMcpToken(id: number): Promise<void> {
  await mcpWrite(`/api/mcp/tokens/${id}`, 'DELETE')
}

// ============================================================================
// Account reset (desktop window only)
// ============================================================================

/** What the desktop window answers to a reset request. */
export interface ResetAccountOutcome {
  status: 'reset' | 'cancelled'
  message: string
}

/**
 * Last-resort recovery when both the password and the authenticator are
 * lost. There is no web address for it: the desktop shell asks the trader to
 * confirm in a system dialog and only then resets, so nothing reaching the
 * local server over the network can wipe the account.
 */
export async function resetAccountFromDesktop(): Promise<ResetAccountOutcome> {
  if (!isDesktopShell()) {
    throw new Error('Open The Bridge on this computer to reset your account.')
  }
  try {
    return await invoke<ResetAccountOutcome>('reset_account')
  } catch (e) {
    const message =
      typeof e === 'object' && e !== null && 'message' in e ? String(e.message) : undefined
    throw new Error(message || 'Your account could not be reset. Restart The Bridge and try again.')
  }
}

// ============================================================================
// Start fresh / set a new password (desktop window only)
// ============================================================================

/**
 * Why the trader is starting fresh: the keychain of an earlier build could
 * not be read (or holds no keys for this account), or the password is
 * forgotten. The operation is the same; the server records which.
 */
export type StartFreshReason = 'keychain' | 'forgot'

/** What the desktop window answers to a start-fresh request. */
export interface StartFreshOutcome {
  status: 'done' | 'cancelled'
  message: string
}

/**
 * Keep the account, settings and history and set a new password, dropping
 * what the old keys protected: saved broker keys, the API key (a new one is
 * made) and two-factor login (set up again). There is no web address for
 * it: the desktop shell asks the trader to confirm in a system dialog and
 * only then changes anything, so nothing reaching the local server over the
 * network can replace the password.
 */
export async function startFreshFromDesktop(
  newPassword: string,
  confirmPassword: string,
  reason: StartFreshReason
): Promise<StartFreshOutcome> {
  if (!isDesktopShell()) {
    throw new Error('Open The Bridge on this computer to set a new password.')
  }
  try {
    return await invoke<StartFreshOutcome>('start_fresh', {
      newPassword,
      confirmPassword,
      reason,
    })
  } catch (e) {
    const message =
      typeof e === 'object' && e !== null && 'message' in e ? String(e.message) : undefined
    throw new Error(
      message || 'Your new password could not be set. Restart The Bridge and try again.'
    )
  }
}

// ============================================================================
// The app window's proof (KEY-01)
// ============================================================================

/** The header the app window's sign-in carries its proof in. */
export const WINDOW_PROOF_HEADER = 'X-OpenAlgo-Window'

let windowProof: Promise<string | null> | null = null

/**
 * Headers that show a sign-in comes from the OpenAlgo Desktop window. The
 * server cannot tell the window from a browser tab on the same computer, so
 * the window asks the desktop shell for a proof only it can get (new at
 * every start) and sends it with the password. Only such a sign-in moves an
 * earlier build's keys and is told why keys could not be opened. A browser
 * tab sends nothing and gets the ordinary answers.
 */
export async function appWindowHeaders(): Promise<Record<string, string>> {
  if (!isDesktopShell()) return {}
  windowProof ??= invoke<string>('window_proof').catch(() => {
    windowProof = null
    return null
  })
  const proof = await windowProof
  return proof ? { [WINDOW_PROOF_HEADER]: proof } : {}
}

/** Forget the cached proof (tests). */
export function resetWindowProofForTests(): void {
  windowProof = null
}

/** What the sign-in answers when the keys of an earlier build cannot be read. */
export type KeyRecovery = 'retry_keychain' | 'start_fresh'

/** The recovery choices of a sign-in answer, or null for an ordinary one. */
export function keyRecoveryOf(body: unknown): KeyRecovery[] | null {
  const b = body as { recovery?: unknown } | null
  if (!b || !Array.isArray(b.recovery)) return null
  const known = b.recovery.filter(
    (r): r is KeyRecovery => r === 'retry_keychain' || r === 'start_fresh'
  )
  return known.length ? known : null
}

// ============================================================================
// In-app updates: the restart (desktop window only)
// ============================================================================

/** What the desktop window answers to a restart into the downloaded update. */
export interface RestartOutcome {
  /** confirm: something new runs since the trader confirmed; show `warning`. */
  status: 'restarting' | 'confirm' | 'refused' | 'failed'
  message: string | null
  activity: UpdateActivity | null
  warning: UpdateWarning | null
}

/**
 * Install the downloaded, verified update and restart into it. Only the
 * desktop window can: the local server downloads and checks the update, but
 * replacing the app and restarting is the shell's job. `confirmed` is the
 * activity the trader saw in the warning, if there was one.
 */
export async function restartToUpdate(confirmed?: UpdateActivity | null): Promise<RestartOutcome> {
  if (!isDesktopShell()) {
    return {
      status: 'refused',
      message: 'Open The Bridge on this computer to finish the update.',
      activity: null,
      warning: null,
    }
  }
  try {
    return await invoke<RestartOutcome>('restart_to_update', { confirmed: confirmed ?? null })
  } catch (e) {
    const message =
      typeof e === 'object' && e !== null && 'message' in e ? String(e.message) : undefined
    return {
      status: 'failed',
      message: message || 'The Bridge could not restart to install the update. Try again.',
      activity: null,
      warning: null,
    }
  }
}

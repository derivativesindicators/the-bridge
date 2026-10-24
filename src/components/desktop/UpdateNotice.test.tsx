import { act, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { UpdateActivity, UpdateStatus } from '@/api/updates'
import { useUpdateStore } from '@/stores/updateStore'
import { UPDATE_STATUS_EVENT, UpdateNotice } from './UpdateNotice'

const api = vi.hoisted(() => ({
  status: vi.fn(),
  check: vi.fn(),
  install: vi.fn(),
  cancel: vi.fn(),
}))
vi.mock('@/api/updates', () => ({ updatesApi: api }))

const shell = vi.hoisted(() => ({
  desktop: true,
  isDesktopShell: vi.fn(),
  openExternal: vi.fn(),
  restartToUpdate: vi.fn(),
}))
vi.mock('@/lib/desktop', () => ({
  isDesktopShell: () => shell.desktop,
  openExternal: shell.openExternal,
  restartToUpdate: shell.restartToUpdate,
}))

const socket = vi.hoisted(() => {
  const handlers = new Map<string, (payload: unknown) => void>()
  return {
    handlers,
    on: vi.fn((event: string, fn: (payload: unknown) => void) => handlers.set(event, fn)),
    off: vi.fn((event: string) => handlers.delete(event)),
  }
})
vi.mock('@/components/socket/SocketProvider', () => ({
  useSocketContext: () => ({ socket, playAlertSound: () => {} }),
}))

const NOTES = '## 1.1.0\n\n- Faster option chain.'

function status(over: Partial<UpdateStatus> = {}): UpdateStatus {
  return {
    current_version: '1.0.0',
    state: 'available',
    message: null,
    progress: null,
    offer: {
      version: '1.1.0',
      notes: NOTES,
      published_at: '2026-10-15T04:30:00Z',
      release_url: 'https://github.com/marketcalls/openalgo-desktop/releases/tag/v1.1.0',
    },
    package: 'mac_app',
    can_install: true,
    manual_reason: null,
    releases_url: 'https://github.com/marketcalls/openalgo-desktop/releases',
    checked_at: '2026-10-15T05:00:00Z',
    ...over,
  }
}

const LIVE: UpdateActivity = {
  live_strategies: 1,
  live_openscript_runs: 0,
  live_scalping_stops: 0,
  live_open_orders: 2,
  live_orders_unknown: false,
  unsettled_orders: 0,
  checks_failed: false,
  sandbox_strategies: 0,
  sandbox_openscript_runs: 0,
  sandbox_scalping_stops: 0,
  sandbox_open_orders: 0,
}

const initial = useUpdateStore.getState()

beforeEach(() => {
  vi.clearAllMocks()
  socket.handlers.clear()
  shell.desktop = true
  useUpdateStore.setState(initial, true)
})

describe('UpdateNotice', () => {
  it('offers a newer version in a banner and shows its release notes', async () => {
    api.status.mockResolvedValue(status())
    const user = userEvent.setup()
    render(<UpdateNotice />)

    const banner = await screen.findByRole('region', { name: 'Update available' })
    expect(banner).toHaveTextContent('The Bridge 1.1.0 is available.')
    expect(banner).toHaveTextContent('You have version 1.0.0.')

    await user.click(within(banner).getByRole('button', { name: 'See what is new' }))
    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByRole('region', { name: 'Release notes' })).toHaveTextContent(
      'Faster option chain.'
    )
    expect(dialog).toHaveTextContent('Nothing is downloaded until you choose Update.')
    // Nothing was asked of the server beyond the status.
    expect(api.install).not.toHaveBeenCalled()
  })

  it('names Live trading before installing and sends the confirmation back', async () => {
    api.status.mockResolvedValue(status())
    api.install
      .mockResolvedValueOnce({
        status: 'confirm',
        activity: LIVE,
        warning: {
          level: 'live',
          title: 'Live trading is running',
          intro: 'Updating restarts The Bridge.',
          items: ['1 running Live strategy.', '2 open orders at Zerodha.'],
          advice: 'Update now only if that is acceptable.',
        },
      })
      .mockResolvedValueOnce({ status: 'success', data: status({ state: 'downloading' }) })
    const user = userEvent.setup()
    render(<UpdateNotice />)
    await user.click(await screen.findByRole('button', { name: 'See what is new' }))
    await user.click(screen.getByRole('button', { name: 'Update' }))

    const dialog = await screen.findByRole('dialog')
    expect(dialog).toHaveTextContent('Live trading is running')
    expect(dialog).toHaveTextContent('2 open orders at Zerodha.')
    expect(api.install).toHaveBeenLastCalledWith(undefined)

    await user.click(screen.getByRole('button', { name: 'Update and restart anyway' }))
    expect(api.install).toHaveBeenLastCalledWith(LIVE)
    expect(useUpdateStore.getState().confirmed).toEqual(LIVE)
    // No restart before the download is verified.
    expect(shell.restartToUpdate).not.toHaveBeenCalled()
  })

  it('restarts with the confirmation once the verified update is pushed as ready', async () => {
    api.status.mockResolvedValue(status())
    api.install.mockResolvedValue({ status: 'success', data: status({ state: 'downloading' }) })
    shell.restartToUpdate.mockResolvedValue({
      status: 'restarting',
      message: null,
      activity: null,
      warning: null,
    })
    const user = userEvent.setup()
    render(<UpdateNotice />)
    await user.click(await screen.findByRole('button', { name: 'See what is new' }))
    await user.click(screen.getByRole('button', { name: 'Update' }))
    await waitFor(() => expect(socket.handlers.has(UPDATE_STATUS_EVENT)).toBe(true))

    act(() => {
      socket.handlers.get(UPDATE_STATUS_EVENT)?.(
        status({ state: 'downloading', progress: { received: 50, total: 200 } })
      )
    })
    expect(await screen.findByRole('progressbar', { name: 'Download progress' })).toHaveAttribute(
      'aria-valuenow',
      '25'
    )
    expect(shell.restartToUpdate).not.toHaveBeenCalled()

    act(() => {
      socket.handlers.get(UPDATE_STATUS_EVENT)?.(status({ state: 'ready' }))
    })
    await waitFor(() => expect(shell.restartToUpdate).toHaveBeenCalledTimes(1))
    expect(shell.restartToUpdate).toHaveBeenCalledWith(null)
  })

  it('asks again when new Live activity appeared before the restart', async () => {
    api.status.mockResolvedValue(status({ state: 'ready' }))
    shell.restartToUpdate
      .mockResolvedValueOnce({
        status: 'confirm',
        message: null,
        activity: LIVE,
        warning: {
          level: 'live',
          title: 'Live trading is running',
          intro: 'Updating restarts The Bridge.',
          items: ['2 open orders at Zerodha.'],
          advice: 'Update now only if that is acceptable.',
        },
      })
      .mockResolvedValueOnce({ status: 'restarting', message: null, activity: null, warning: null })
    const user = userEvent.setup()
    render(<UpdateNotice />)
    await user.click(await screen.findByRole('button', { name: 'See what is new' }))
    await user.click(screen.getByRole('button', { name: 'Restart now' }))
    expect(await screen.findByText('2 open orders at Zerodha.')).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Update and restart anyway' }))
    expect(shell.restartToUpdate).toHaveBeenLastCalledWith(LIVE)
  })

  it('sandbox-only activity gets the lighter confirmation', async () => {
    api.status.mockResolvedValue(status())
    api.install.mockResolvedValueOnce({
      status: 'confirm',
      activity: { ...LIVE, live_strategies: 0, live_open_orders: 0, sandbox_open_orders: 1 },
      warning: {
        level: 'sandbox',
        title: 'Sandbox activity is running',
        intro: 'Updating restarts The Bridge.',
        items: ['1 open sandbox order.'],
        advice: 'Nothing is sent to your broker.',
      },
    })
    const user = userEvent.setup()
    render(<UpdateNotice />)
    await user.click(await screen.findByRole('button', { name: 'See what is new' }))
    await user.click(screen.getByRole('button', { name: 'Update' }))
    expect(await screen.findByText('Sandbox activity is running')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Update and restart' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Update and restart anyway' })).toBeNull()
  })

  it('a .deb copy is told how to update and given the release page', async () => {
    api.status.mockResolvedValue(
      status({
        package: 'linux_deb',
        can_install: false,
        manual_reason:
          'This copy was installed from a .deb package, which The Bridge cannot update by itself.',
      })
    )
    const user = userEvent.setup()
    render(<UpdateNotice />)
    await user.click(await screen.findByRole('button', { name: 'See what is new' }))
    const dialog = await screen.findByRole('dialog')
    expect(dialog).toHaveTextContent('installed from a .deb package')
    expect(within(dialog).queryByRole('button', { name: 'Update' })).toBeNull()

    await user.click(within(dialog).getByRole('button', { name: 'Open the release page' }))
    expect(shell.openExternal).toHaveBeenCalledWith(
      'https://github.com/marketcalls/openalgo-desktop/releases/tag/v1.1.0'
    )
  })

  it('Later hides the banner for that version', async () => {
    api.status.mockResolvedValue(status())
    const user = userEvent.setup()
    render(<UpdateNotice />)
    const banner = await screen.findByRole('region', { name: 'Update available' })
    await user.click(within(banner).getByRole('button', { name: 'Later' }))
    expect(screen.queryByRole('region', { name: 'Update available' })).toBeNull()
    expect(useUpdateStore.getState().dismissedVersion).toBe('1.1.0')
  })

  it('Check for Updates says when this is the newest version', async () => {
    api.status.mockResolvedValue(status({ state: 'not_checked', offer: null }))
    api.check.mockResolvedValue(status({ state: 'up_to_date', offer: null }))
    render(<UpdateNotice />)
    await act(async () => {
      await useUpdateStore.getState().checkNow()
    })
    const dialog = await screen.findByRole('dialog')
    expect(dialog).toHaveTextContent('You are on the latest version, 1.0.0.')
    expect(screen.queryByRole('region', { name: 'Update available' })).toBeNull()
  })

  it('a browser tab is pointed at the desktop window instead of an Update button', async () => {
    shell.desktop = false
    api.status.mockResolvedValue(status())
    const user = userEvent.setup()
    render(<UpdateNotice />)
    await user.click(await screen.findByRole('button', { name: 'See what is new' }))
    const dialog = await screen.findByRole('dialog')
    expect(dialog).toHaveTextContent('Open The Bridge on this computer to install it.')
    expect(within(dialog).queryByRole('button', { name: 'Update' })).toBeNull()
  })
})

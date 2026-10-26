/**
 * A squared-off position offers no Close button.
 *
 * Issue #2054: the red X was rendered on every row, including the rows a
 * broker keeps reporting after a position is closed - quantity 0, realised
 * P&L still attached. Clicking it sent a placesmartorder the engine then
 * refused, logging "No OpenPosition Found. Not placing Exit order.", after the
 * alert channels had already announced it.
 *
 * The page's own header already works this way: Close All is disabled when
 * `stats.total` is 0, and that count deliberately excludes zero-quantity rows.
 * So does the trading dock's position table, whose header comment records the
 * rule - zero-quantity rows "stay visible with their realised figure and no
 * Close". Only this table had missed it.
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor, within } from '@/test/test-utils'
import type { Position } from '@/types/trading'

const mocks = vi.hoisted(() => ({
  getPositions: vi.fn(),
  closePosition: vi.fn(),
  closeAllPositions: vi.fn(),
  openLegs: vi.fn(),
  reconcile: vi.fn(),
}))

vi.mock('@/api/desk', () => ({
  fetchOpenStrategyLegs: mocks.openLegs,
  reconcileStrategyLegs: mocks.reconcile,
}))

vi.mock('@/api/trading', () => ({
  tradingApi: {
    getPositions: mocks.getPositions,
    closePosition: mocks.closePosition,
    closeAllPositions: mocks.closeAllPositions,
  },
}))

vi.mock('@/stores/authStore', () => ({
  useAuthStore: () => ({ apiKey: 'test-api-key', user: { broker: 'zerodha' } }),
}))

vi.mock('@/stores/themeStore', () => ({
  onModeChange: () => () => {},
  useThemeStore: (sel: (s: { appMode: string }) => unknown) => sel({ appMode: 'analyzer' }),
}))

// The live-price hook owns the WebSocket. Pass the positions straight through
// so the table renders exactly what the API returned.
vi.mock('@/hooks/useLivePrice', () => ({
  useLivePrice: (positions: Position[]) => ({
    data: positions,
    isLive: false,
    isPaused: false,
  }),
}))

const eventRefresh = vi.hoisted(() => vi.fn())
vi.mock('@/hooks/useOrderEventRefresh', () => ({
  useOrderEventRefresh: (...args: unknown[]) => eventRefresh(...args),
}))
vi.mock('@/hooks/usePageVisibility', () => ({
  usePageVisibility: () => ({ isVisible: true, wasHidden: false, timeSinceHidden: 0 }),
}))
vi.mock('@/hooks/useSupportedExchanges', () => ({
  useSupportedExchanges: () => ({ isCrypto: false }),
}))

import Positions from './Positions'

const OPEN_POSITION: Position = {
  symbol: 'NIFTY25SEP2625000CE',
  exchange: 'NFO',
  product: 'NRML',
  quantity: -75,
  average_price: 120.5,
  ltp: 98.25,
  pnl: 1668.75,
} as Position

const CLOSED_POSITION: Position = {
  symbol: 'BANKNIFTY25SEP2654000PE',
  exchange: 'NFO',
  product: 'NRML',
  quantity: 0,
  average_price: 210.1,
  ltp: 185.4,
  pnl: -740.0,
} as Position

async function renderPositions(positions: Position[]) {
  mocks.getPositions.mockResolvedValue({ status: 'success', data: positions })
  render(<Positions />)
  // The first row to arrive tells us the fetch has resolved and painted.
  await screen.findByText(positions[0].symbol)
}

describe('Positions close button', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    localStorage.clear()
  })

  it("refreshes on the broker's later status for a resting order (order_update)", async () => {
    await renderPositions([OPEN_POSITION])
    expect(eventRefresh).toHaveBeenCalledWith(
      expect.any(Function),
      expect.objectContaining({ events: expect.arrayContaining(['order_update']) })
    )
  })

  it('offers Close on a position that is still open', async () => {
    await renderPositions([OPEN_POSITION])

    expect(
      screen.getByRole('button', { name: `Close ${OPEN_POSITION.symbol} position` })
    ).toBeInTheDocument()
  })

  it('THE DEFECT: offers no Close on a position already squared off', async () => {
    await renderPositions([CLOSED_POSITION])

    expect(
      screen.queryByRole('button', { name: `Close ${CLOSED_POSITION.symbol} position` })
    ).not.toBeInTheDocument()
  })

  it('leaves the open row actionable when both are on the page', async () => {
    await renderPositions([OPEN_POSITION, CLOSED_POSITION])

    expect(
      screen.getByRole('button', { name: `Close ${OPEN_POSITION.symbol} position` })
    ).toBeInTheDocument()
    expect(
      screen.queryByRole('button', { name: `Close ${CLOSED_POSITION.symbol} position` })
    ).not.toBeInTheDocument()
  })

  it('never sends a close request for a squared-off position', async () => {
    await renderPositions([CLOSED_POSITION])

    // Nothing on the row can reach the endpoint, so the engine is never asked
    // to exit a position that is not there, and no alert can be raised for it.
    expect(mocks.closePosition).not.toHaveBeenCalled()
  })

  it('keeps the closed row and its realised P&L visible', async () => {
    await renderPositions([CLOSED_POSITION])

    // Hiding the row would lose the realised figure, which is the reason the
    // broker still reports it. Only the action goes.
    expect(screen.getByText(CLOSED_POSITION.symbol)).toBeInTheDocument()
  })

  it('does not colour a zero quantity as a short', async () => {
    await renderPositions([CLOSED_POSITION])

    // Symbol, exchange, product, quantity - the fourth cell of the row.
    const row = screen.getByText(CLOSED_POSITION.symbol).closest('tr')
    expect(row).not.toBeNull()
    const quantityCell = within(row as HTMLElement).getAllByRole('cell')[3]

    expect(quantityCell).toHaveTextContent('0')
    expect(quantityCell.className).toContain('text-muted-foreground')
    expect(quantityCell.className).not.toContain('text-red-600')
  })

  it('treats a string zero quantity as closed, not as open', async () => {
    // The API does not always send a number. Zerodha's position mapping
    // defaults quantity to the string "0", and a strict `!== 0` reads that as
    // an open position - putting the button back on the exact row this guard
    // exists to clear.
    await renderPositions([{ ...CLOSED_POSITION, quantity: '0' } as unknown as Position])

    expect(
      screen.queryByRole('button', { name: `Close ${CLOSED_POSITION.symbol} position` })
    ).not.toBeInTheDocument()
  })

  it('still offers Close when a live quantity arrives as a string', async () => {
    await renderPositions([{ ...OPEN_POSITION, quantity: '-75' } as unknown as Position])

    expect(
      screen.getByRole('button', { name: `Close ${OPEN_POSITION.symbol} position` })
    ).toBeInTheDocument()
  })

  it('disables Close All when every position is squared off', async () => {
    await renderPositions([CLOSED_POSITION])

    expect(screen.getByRole('button', { name: /Close All/ })).toBeDisabled()
  })
})

describe('Positions close all with running strategies (The Bridge 1.0.11)', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    localStorage.clear()
  })

  it('warns when strategy legs are open and offers Cancel and Close anyway', async () => {
    mocks.openLegs.mockResolvedValue(2)
    await renderPositions([OPEN_POSITION])
    fireEvent.click(screen.getByRole('button', { name: /Close All/ }))
    const warning = await screen.findByTestId('strategy-legs-warning')
    expect(warning).toHaveTextContent(
      "2 positions belong to running strategies; closing them here won't update those strategies. Use Strategy Positions → Close all open positions instead."
    )
    expect(mocks.openLegs).toHaveBeenCalledWith('sandbox')
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Close anyway' })).toBeInTheDocument()
  })

  it('Cancel sends nothing', async () => {
    mocks.openLegs.mockResolvedValue(1)
    await renderPositions([OPEN_POSITION])
    fireEvent.click(screen.getByRole('button', { name: /Close All/ }))
    await screen.findByTestId('strategy-legs-warning')
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(mocks.closeAllPositions).not.toHaveBeenCalled()
  })

  it('Close anyway closes, then reconciles the strategy legs', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    try {
      mocks.openLegs.mockResolvedValue(1)
      mocks.closeAllPositions.mockResolvedValue({ status: 'success' })
      await renderPositions([OPEN_POSITION])
      fireEvent.click(screen.getByRole('button', { name: /Close All/ }))
      await screen.findByTestId('strategy-legs-warning')
      fireEvent.click(screen.getByRole('button', { name: 'Close anyway' }))
      await waitFor(() => expect(mocks.closeAllPositions).toHaveBeenCalled())
      vi.advanceTimersByTime(2000)
      await waitFor(() => expect(mocks.reconcile).toHaveBeenCalledWith('sandbox'))
    } finally {
      vi.useRealTimers()
    }
  })

  it('keeps the plain confirm when no strategy holds a position', async () => {
    mocks.openLegs.mockResolvedValue(0)
    await renderPositions([OPEN_POSITION])
    fireEvent.click(screen.getByRole('button', { name: /Close All/ }))
    await screen.findByText('Close All Positions?')
    expect(screen.queryByTestId('strategy-legs-warning')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Close anyway' })).toBeNull()
  })
})

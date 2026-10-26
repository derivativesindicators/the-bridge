import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { contractText, lotsText } from '@/api/desk'

const fetchDesk = vi.fn()
const fetchDeskMargin = vi.fn()
vi.mock('@/api/desk', async (orig) => ({
  ...(await orig<typeof import('@/api/desk')>()),
  fetchDesk: (m: string) => fetchDesk(m),
  fetchDeskMargin: (m: string) => fetchDeskMargin(m),
  closeStrategyPositions: (id: number) => closeStrategyPositions(id),
  closeAllStrategyPositions: () => closeAllStrategyPositions(),
}))
const closeStrategyPositions = vi.fn()
const closeAllStrategyPositions = vi.fn()

import StrategyDesk from './Desk'

const desk = {
  mode: 'sandbox',
  book_available: true,
  totals: { realized: 1500, unrealized: -300, total: 1200 },
  strategies: [
    {
      id: 7,
      name: 'Nifty straddle',
      status: 'running',
      product: 'NRML',
      realized: 1500,
      unrealized: -300,
      total: 1200,
      positions: [
        {
          symbol: 'NIFTY28OCT2625000CE',
          exchange: 'NFO',
          product: 'NRML',
          quantity: -75,
          average_price: 100,
          ltp: 104,
          mtm: -300,
          leg_id: 1,
          side: 'short',
          lot_size: 75,
          lots: 1,
          expiry: '28-OCT-26',
          strike: 25000,
          instrument_type: 'CE',
        },
      ],
      closed_today: [
        {
          run_id: 1,
          leg_id: 2,
          symbol: 'NIFTY28OCT2625000PE',
          exchange: 'NFO',
          side: 'long',
          qty: 75,
          entry_price: 100,
          exit_price: 120,
          exit_time: '2026-10-26 05:00:00',
          exit_kind: 'exit_target',
          pnl: 1500,
        },
      ],
    },
  ],
}

function view() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <StrategyDesk />
      </MemoryRouter>
    </QueryClientProvider>
  )
}

beforeEach(() => {
  fetchDesk.mockReset().mockResolvedValue(desk)
  fetchDeskMargin.mockReset().mockResolvedValue({
    mode: 'sandbox',
    estimate: true,
    available: true,
    combined: 52000,
    sum_of_legs: 140000,
    legs: 2,
    per_strategy: { '7': 52000 },
  })
})

describe('Strategy desk', () => {
  it('shows positions, totals and the combined margin, with Live off by default', async () => {
    localStorage.clear()
    view()
    expect(await screen.findByText('NIFTY28OCT2625000CE')).toBeTruthy()
    expect(screen.getAllByRole('link', { name: 'Nifty straddle' })[0].getAttribute('href')).toBe(
      '/strategy/7'
    )
    expect(screen.getByText('1 lot (75)')).toBeTruthy()
    expect(screen.getByText('Total margin used')).toBeTruthy()
    expect(screen.queryByText(/estimate/i)).toBeNull()
    expect(screen.getByRole('switch', { name: 'Live' }).getAttribute('aria-checked')).toBe('false')
    expect(screen.queryByText(/sandbox|paper/i)).toBeNull()
    await waitFor(() => expect(screen.getByText(/Legs individually/)).toBeTruthy())
    expect(screen.getByText('Closed today (1)')).toBeTruthy()
    expect(fetchDesk).toHaveBeenCalledWith('sandbox')
  })

  it('switches to Live, remembers it, and collapses closed trades', async () => {
    localStorage.clear()
    view()
    await screen.findByText('NIFTY28OCT2625000CE')
    fireEvent.click(screen.getByRole('switch', { name: 'Live' }))
    await waitFor(() => expect(fetchDesk).toHaveBeenCalledWith('live'))
    expect(screen.getByRole('switch', { name: 'Live' }).getAttribute('aria-checked')).toBe('true')
    expect(localStorage.getItem('bridge.strategyPositions.mode')).toBe('live')
    fireEvent.click(screen.getByRole('button', { name: /Closed today/ }))
    expect(screen.queryByText('exit_target')).toBeNull()
  })

  it('defaults to Live when a strategy is live-enabled', async () => {
    localStorage.clear()
    fetchDesk.mockResolvedValue({ ...desk, any_live_enabled: true })
    view()
    await waitFor(() => expect(fetchDesk).toHaveBeenCalledWith('live'))
  })

  it('degrades when the broker cannot price margin', async () => {
    localStorage.clear()
    fetchDeskMargin.mockResolvedValue({
      mode: 'sandbox',
      estimate: true,
      available: false,
      message: 'Not logged in',
      combined: null,
      sum_of_legs: null,
      legs: 1,
      per_strategy: {},
    })
    view()
    expect(await screen.findByText(/Not available from the broker/)).toBeTruthy()
  })
})

describe('desk text helpers', () => {
  it('formats lots and contracts', () => {
    expect(lotsText({ quantity: -150, lots: 2, lot_size: 75 })).toBe('2 lots (150)')
    expect(lotsText({ quantity: 10, lots: null, lot_size: null })).toBe('10')
    expect(contractText({ strike: 25000, instrument_type: 'CE', expiry: '28-OCT-26' })).toBe(
      '25000 · CE · 28-OCT-26'
    )
  })
})

describe('1.0.11 side colours, Close positions and Close all', () => {
  it('colours by market direction: short CE red, long PE (closed) red', async () => {
    view()
    const badge = await screen.findByText('Short', { selector: 'span,div' })
    expect(badge.className).toContain('bg-red-600')
    const closed = screen.getAllByText('Long').find((e) => e.tagName === 'TD')
    expect(closed?.className).toContain('text-red-600')
  })

  it('Close positions confirms with the strategy name and keeps it running', async () => {
    closeStrategyPositions.mockReset().mockResolvedValue({ id: 7, ok: true, message: 'Closed 1' })
    view()
    fireEvent.click(await screen.findByRole('button', { name: 'Close positions' }))
    expect(await screen.findByText(/Close positions of “Nifty straddle”\?/)).toBeTruthy()
    expect(screen.getByText(/strategy keeps running/)).toBeTruthy()
    const buttons = screen.getAllByRole('button', { name: 'Close positions' })
    fireEvent.click(buttons[buttons.length - 1])
    await waitFor(() => expect(closeStrategyPositions).toHaveBeenCalledWith(7))
    expect(await screen.findByText('Closed 1')).toBeTruthy()
  })

  it('Close all states both books and the counts, and reports per strategy', async () => {
    closeAllStrategyPositions.mockReset().mockResolvedValue({
      status: 'success',
      closed_strategies: 1,
      failed: 1,
      results: [
        { id: 7, name: 'Nifty straddle', mode: 'sandbox', ok: true, message: 'closed ok' },
        { id: 8, name: 'Bank live', mode: 'live', ok: false, error: 'broker refused' },
      ],
    })
    fetchDesk.mockImplementation(async (m: string) =>
      m === 'live' ? { ...desk, mode: 'live', strategies: [] } : desk
    )
    view()
    fireEvent.click(await screen.findByRole('button', { name: 'Close all open positions' }))
    expect(await screen.findByText(/BOTH the sandbox and the live book/)).toBeTruthy()
    expect(screen.getByText(/1 open position across 1 strategy/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Close all' }))
    await waitFor(() => expect(closeAllStrategyPositions).toHaveBeenCalled())
    expect(await screen.findByText(/Bank live \(live\): broker refused/)).toBeTruthy()
    expect(screen.getByText(/Nifty straddle \(sandbox\): closed ok/)).toBeTruthy()
  })
})

describe('1.0.11 Net positions view', () => {
  const net = [
    {
      symbol: 'NIFTY27OCT2623000CE',
      exchange: 'NFO',
      product: 'NRML',
      quantity: 750,
      average_price: 90,
      ltp: 100,
      realized: 500,
      unrealized: 7500,
      pnl: 8000,
      change_percent: 11.11,
      buy_quantity: 1500,
      sell_quantity: 750,
      instrument_type: 'CE',
    },
    {
      symbol: 'NIFTY27OCT2622700PE',
      exchange: 'NFO',
      product: 'NRML',
      quantity: -75,
      average_price: 50,
      ltp: 40,
      realized: 0,
      unrealized: 750,
      pnl: 750,
      buy_quantity: 0,
      sell_quantity: 75,
      instrument_type: 'PE',
      mismatch: true,
      broker_quantity: 0,
    },
    {
      symbol: 'NIFTY27OCT2623100CE',
      exchange: 'NFO',
      product: 'MIS',
      quantity: 0,
      average_price: null,
      ltp: 20,
      realized: -200,
      unrealized: null,
      pnl: -200,
      buy_quantity: 75,
      sell_quantity: 75,
    },
  ]

  it('nets by contract with no strategy names, Kite columns, colours and a total', async () => {
    localStorage.removeItem('bridge.strategyPositions.view')
    fetchDesk.mockResolvedValue({ ...desk, net, net_total: 8550 })
    view()
    fireEvent.click(await screen.findByRole('tab', { name: 'Net' }))
    const rows = await screen.findAllByTestId('net-row')
    expect(rows).toHaveLength(3)
    expect(screen.queryByText('Nifty straddle')).not.toBeInTheDocument()
    for (const h of ['Product', 'Instrument', 'Qty.', 'Avg.', 'LTP', 'Chg.']) {
      expect(screen.getByText(h)).toBeInTheDocument()
    }
    expect(screen.getByText('+750').className).toContain('text-green')
    // Short PE gains as the market rises: green.
    expect(screen.getByText('-75').className).toContain('text-green')
    // The flat row is last and shows realized only.
    expect(rows[2].textContent).toContain('NIFTY27OCT2623100CE')
    expect(screen.getByText('⚠ broker')).toBeInTheDocument()
    expect(screen.getByTestId('net-total').textContent).toContain('8,550')
    // The single Live switch stays, no mode words on rows.
    expect(screen.getByRole('switch')).toBeInTheDocument()
    expect(rows.map((r) => r.textContent).join(' ')).not.toMatch(/sandbox|paper/i)
    localStorage.removeItem('bridge.strategyPositions.view')
  })
})

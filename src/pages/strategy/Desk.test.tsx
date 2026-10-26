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
}))

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

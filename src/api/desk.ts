// The Bridge 1.0.10: the Strategy desk API (every strategy's open positions,
// today's P&L and the margin they block, for one mode).
import { webClient } from '@/api/client'

export type DeskMode = 'sandbox' | 'live'

export interface DeskPosition {
  symbol: string
  exchange: string
  product: string
  quantity: number
  average_price: number | null
  ltp: number | null
  mtm: number | null
  leg_id?: number | null
  run_id?: number | null
  source?: string
  side: 'long' | 'short' | 'flat'
  lot_size: number | null
  lots: number | null
  expiry?: string
  strike?: number | null
  instrument_type?: string
}

export interface DeskTrip {
  run_id: number
  leg_id: number
  symbol: string
  exchange: string
  side: 'long' | 'short'
  qty: number
  entry_price: number
  exit_price: number
  exit_time: string
  exit_kind: string
  pnl: number
}

export interface DeskStrategy {
  id: number
  name: string
  status: string
  product: string
  positions: DeskPosition[]
  closed_today: DeskTrip[]
  realized: number
  unrealized: number
  total: number
}

export interface DeskData {
  mode: DeskMode
  book_available: boolean
  any_live_enabled?: boolean
  strategies: DeskStrategy[]
  totals: { realized: number; unrealized: number; total: number }
}

export interface DeskMargin {
  mode: DeskMode
  estimate: boolean
  available: boolean
  message?: string | null
  combined: number | null
  sum_of_legs: number | null
  legs: number
  per_strategy: Record<string, number>
}

export async function fetchDesk(mode: DeskMode): Promise<DeskData> {
  const r = await webClient.get<{ status: string; data: DeskData; message?: string }>(
    '/strategy/api/desk',
    { params: { mode } }
  )
  if (r.data?.status !== 'success') throw new Error(r.data?.message ?? 'Could not read the desk')
  return r.data.data
}

export async function fetchDeskMargin(mode: DeskMode): Promise<DeskMargin> {
  const r = await webClient.get<{ status: string; data: DeskMargin; message?: string }>(
    '/strategy/api/desk/margin',
    { params: { mode } }
  )
  if (r.data?.status !== 'success') throw new Error(r.data?.message ?? 'Margin is not available')
  return r.data.data
}

/** "1 lot (75)" style quantity text. */
export function lotsText(p: Pick<DeskPosition, 'quantity' | 'lots' | 'lot_size'>): string {
  const units = Math.abs(p.quantity)
  if (p.lots == null || p.lot_size == null || p.lot_size <= 1) return `${units}`
  return `${p.lots} lot${p.lots === 1 ? '' : 's'} (${units})`
}

/** Contract description: strike, option type and expiry when known. */
export function contractText(
  p: Pick<DeskPosition, 'strike' | 'instrument_type' | 'expiry'>
): string {
  const parts: string[] = []
  if (p.strike) parts.push(String(p.strike))
  if (p.instrument_type && p.instrument_type !== 'EQ') parts.push(p.instrument_type)
  if (p.expiry) parts.push(p.expiry)
  return parts.join(' · ')
}

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
  run_mode?: string | null
  positions: DeskPosition[]
  closed_today: DeskTrip[]
  realized: number
  unrealized: number
  total: number
}

/** A Kite-style net row: every strategy's fills netted by contract. */
export interface NetPosition {
  symbol: string
  exchange: string
  product: string
  quantity: number
  average_price: number | null
  ltp: number | null
  realized: number
  unrealized: number | null
  pnl: number
  change_percent?: number | null
  buy_quantity: number
  sell_quantity: number
  lots?: number | null
  lot_size?: number | null
  instrument_type?: string
  broker_quantity?: number
  mismatch?: boolean
}

export interface DeskData {
  mode: DeskMode
  book_available: boolean
  net?: NetPosition[]
  net_total?: number
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

// The Bridge 1.0.11: close a strategy's open positions now (it keeps running).

export interface ClosePositionsResult {
  id: number
  name?: string
  mode?: string | null
  ok: boolean
  closed?: number
  error?: string | null
  message?: string
}

function errorBody(e: unknown): ClosePositionsResult | undefined {
  return (e as { response?: { data?: ClosePositionsResult } })?.response?.data
}

/** Square off this strategy's own open legs at MARKET; the strategy keeps running. */
export async function closeStrategyPositions(id: number): Promise<ClosePositionsResult> {
  try {
    const r = await webClient.post<ClosePositionsResult>(
      `/strategy/api/strategies/${id}/close_positions`,
      {}
    )
    return r.data
  } catch (e) {
    const b = errorBody(e)
    return { ...(b ?? {}), id, ok: false, error: b?.error ?? b?.message ?? 'Close failed' }
  }
}

export interface CloseAllReply {
  status: string
  results: ClosePositionsResult[]
  closed_strategies: number
  failed: number
}

/** Close every open position of every running strategy, sandbox and live. */
export async function closeAllStrategyPositions(): Promise<CloseAllReply> {
  try {
    const r = await webClient.post<CloseAllReply>('/strategy/api/desk/close_all', {})
    return r.data
  } catch (e) {
    throw new Error(errorBody(e)?.message ?? 'Close all could not run')
  }
}

/** The Bridge 1.0.11: open strategy legs in one book (0 when unreadable). */
export async function fetchOpenStrategyLegs(mode: DeskMode): Promise<number> {
  try {
    const r = await webClient.get<{ status: string; open_legs?: number }>(
      '/strategy/api/desk/open_legs',
      { params: { mode } }
    )
    return r.data?.open_legs ?? 0
  } catch {
    return 0
  }
}

/** Mark strategy legs whose contract is now flat in the book as closed. */
export async function reconcileStrategyLegs(mode: DeskMode): Promise<void> {
  try {
    await webClient.post('/strategy/api/desk/reconcile', { mode })
  } catch {
    // Best effort: the sandbox also reconciles on its own every few seconds.
  }
}

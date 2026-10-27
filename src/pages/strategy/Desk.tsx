// pages/strategy/Desk.tsx
// The Bridge 1.0.10: the Strategy desk. Every open position of every strategy
// in one book (Live switch on: live, off: the strategy sandbox), live LTP and MTM, per-strategy subtotals,
// total day P&L, the margin the positions block, and today's closed trades.
// Strategy-module trades live in their own book, so they are not on the
// Orderbook, Tradebook or Positions pages.

import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, Info } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Link } from 'react-router'
import {
  type CloseAllReply,
  type ClosePositionsResult,
  closeAllStrategyPositions,
  closeStrategyPositions,
  contractText,
  type DeskMargin,
  type DeskMode,
  type DeskStrategy,
  fetchDesk,
  fetchDeskMargin,
  lotsText,
  type NetPosition,
} from '@/api/desk'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { istStamp } from '@/lib/istStamp'
import { sideBadgeClass, sideTextClass } from '@/lib/sideColor'

const inr = (v: number | null | undefined) =>
  v == null || !Number.isFinite(v)
    ? '—'
    : v.toLocaleString('en-IN', { style: 'currency', currency: 'INR', maximumFractionDigits: 2 })
const price = (v: number | null | undefined) =>
  v == null || !Number.isFinite(v) ? '—' : v.toFixed(2)
/** 1.0.12: a price with its fill time (IST) under it. */
function PriceAt({
  value,
  at,
  label,
}: {
  value: number | null | undefined
  at?: string | null
  label: string
}) {
  const when = istStamp(at)
  return (
    <>
      <div>{price(value)}</div>
      {when && (
        <div
          className="text-[10px] text-muted-foreground"
          title={`${label} time`}
          data-testid={`${label.toLowerCase()}-time`}
        >
          {when}
        </div>
      )}
    </>
  )
}

const pnlClass = (v: number | null | undefined) =>
  v == null ? '' : v > 0 ? 'text-green-600' : v < 0 ? 'text-red-600' : ''

function Stat({
  label,
  value,
  sub,
  cls,
  info,
}: {
  label: string
  value: string
  sub?: string
  cls?: string
  info?: string
}) {
  return (
    <Card>
      <CardHeader className="pb-2">
        <CardDescription className="flex items-center gap-1">
          {label}
          {info && (
            <Tooltip>
              <TooltipTrigger asChild>
                <button type="button" aria-label={info} className="text-muted-foreground">
                  <Info className="h-3.5 w-3.5" />
                </button>
              </TooltipTrigger>
              <TooltipContent>{info}</TooltipContent>
            </Tooltip>
          )}
        </CardDescription>
        <CardTitle className={`text-2xl font-mono ${cls ?? ''}`}>{value}</CardTitle>
      </CardHeader>
      {sub && <CardContent className="pt-0 text-xs text-muted-foreground">{sub}</CardContent>}
    </Card>
  )
}

function marginSub(m: DeskMargin | undefined, loading: boolean): string | undefined {
  if (loading && !m) return undefined
  if (!m?.available) return 'Not available from the broker right now'
  return m.sum_of_legs != null ? `Legs individually: ${inr(m.sum_of_legs)}` : undefined
}

const CLOSE_TIP =
  "Book profit/loss on this strategy's open positions now. The strategy keeps running."

/** Close positions: confirm naming the strategy, then square off its own legs; it keeps running. */
function ClosePositionsButton({ s, onDone }: { s: DeskStrategy; onDone: () => void }) {
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<ClosePositionsResult | null>(null)
  const run = async () => {
    setBusy(true)
    setResult(await closeStrategyPositions(s.id))
    setBusy(false)
    onDone()
  }
  return (
    <>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="destructive"
            size="sm"
            className="ml-3 h-6 px-2 text-xs"
            onClick={() => {
              setResult(null)
              setOpen(true)
            }}
          >
            Close positions
          </Button>
        </TooltipTrigger>
        <TooltipContent>{CLOSE_TIP}</TooltipContent>
      </Tooltip>
      <Dialog open={open} onOpenChange={(o) => !busy && setOpen(o)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Close positions of “{s.name}”?</DialogTitle>
            <DialogDescription>
              Squares off this strategy's {s.positions.length} open position
              {s.positions.length === 1 ? '' : 's'} now at MARKET, each for its own quantity
              {s.run_mode === 'live' ? ' at the broker' : ' in the strategy sandbox book'}. The
              strategy keeps running and takes new signals.
            </DialogDescription>
          </DialogHeader>
          {result && (
            <p className={`text-sm ${result.ok ? 'text-green-600' : 'text-red-600'}`}>
              {result.ok ? (result.message ?? 'Closed') : (result.error ?? 'Close failed')}
            </p>
          )}
          <DialogFooter>
            <Button variant="outline" onClick={() => setOpen(false)} disabled={busy}>
              {result ? 'Close' : 'Cancel'}
            </Button>
            {!result && (
              <Button variant="destructive" onClick={run} disabled={busy}>
                {busy ? 'Closing…' : 'Close positions'}
              </Button>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}

/** Close all open positions of every strategy, sandbox and live; all keep running. */
function CloseAllButton({
  counts,
  onDone,
}: {
  counts: { strategies: number; positions: number } | null
  onDone: () => void
}) {
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [reply, setReply] = useState<CloseAllReply | null>(null)
  const [error, setError] = useState<string | null>(null)
  const fire = async () => {
    setBusy(true)
    try {
      setReply(await closeAllStrategyPositions())
    } catch (e) {
      setError((e as Error).message)
    }
    setBusy(false)
    onDone()
  }
  return (
    <>
      <Button
        variant="destructive"
        size="sm"
        onClick={() => {
          setReply(null)
          setError(null)
          setOpen(true)
        }}
      >
        Close all open positions
      </Button>
      <Dialog open={open} onOpenChange={(o) => !busy && setOpen(o)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Close all open positions?</DialogTitle>
            <DialogDescription>
              {counts === null
                ? 'Counting open positions…'
                : `Closes ${counts.positions} open position${counts.positions === 1 ? '' : 's'} across ${counts.strategies} strateg${counts.strategies === 1 ? 'y' : 'ies'}, in BOTH the sandbox and the live book, at MARKET, each for the strategy's own quantity.`}{' '}
              Every strategy keeps running and takes new signals. Other positions in your broker
              account are not touched.
            </DialogDescription>
          </DialogHeader>
          {error && <p className="text-sm text-red-600">{error}</p>}
          {reply && (
            <div className="space-y-1 text-sm">
              <p>
                {reply.results.length === 0
                  ? 'Nothing was open.'
                  : `Closed ${reply.closed_strategies}, failed ${reply.failed}.`}
              </p>
              <ul className="max-h-48 overflow-y-auto">
                {reply.results.map((r) => (
                  <li key={r.id} className={r.ok ? 'text-green-600' : 'text-red-600'}>
                    {r.name ?? `Strategy ${r.id}`}
                    {r.mode ? ` (${r.mode})` : ''}:{' '}
                    {r.ok ? (r.message ?? 'closed') : (r.error ?? 'failed')}
                  </li>
                ))}
              </ul>
            </div>
          )}
          <DialogFooter>
            <Button variant="outline" onClick={() => setOpen(false)} disabled={busy}>
              {reply ? 'Close' : 'Cancel'}
            </Button>
            {!reply && (
              <Button variant="destructive" onClick={fire} disabled={busy || counts === null}>
                {busy ? 'Closing…' : 'Close all'}
              </Button>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}

function StrategyGroup({
  s,
  margin,
  onChanged,
}: {
  s: DeskStrategy
  margin: number | undefined
  onChanged: () => void
}) {
  return (
    <>
      <TableRow className="bg-muted/50">
        <TableCell colSpan={7} className="font-medium">
          <Link to={`/strategy/${s.id}`} className="hover:underline">
            {s.name}
          </Link>{' '}
          <Badge variant="outline" className="ml-2 text-xs">
            {s.status}
          </Badge>
          {margin != null && (
            <span className="ml-3 text-xs text-muted-foreground">Margin {inr(margin)}</span>
          )}
          {s.status === 'running' && s.positions.length > 0 && (
            <ClosePositionsButton s={s} onDone={onChanged} />
          )}
        </TableCell>
        <TableCell className="text-right text-xs text-muted-foreground">
          Realized <span className={pnlClass(s.realized)}>{inr(s.realized)}</span>
        </TableCell>
        <TableCell className={`text-right font-mono font-medium ${pnlClass(s.total)}`}>
          {inr(s.total)}
        </TableCell>
      </TableRow>
      {s.positions.map((p) => (
        <TableRow key={`${s.id}-${p.symbol}-${p.exchange}-${p.product}-${p.leg_id ?? ''}`}>
          <TableCell className="text-xs text-muted-foreground">
            {p.leg_id != null ? `Leg ${p.leg_id}` : '—'}
            {(p.source === 'broker/shared' || p.source === 'local/shared') && (
              <Badge variant="outline" className="ml-1 text-[10px]">
                shared
              </Badge>
            )}
          </TableCell>
          <TableCell>
            <div className="font-mono text-sm">{p.symbol}</div>
            <div className="text-xs text-muted-foreground">
              {p.exchange}
              {contractText(p) ? ` · ${contractText(p)}` : ''}
            </div>
          </TableCell>
          <TableCell>
            <Badge className={`text-xs ${sideBadgeClass(p.side, p.symbol, p.instrument_type)}`}>
              {p.side === 'long' ? 'Long' : 'Short'}
            </Badge>
          </TableCell>
          <TableCell className="text-right font-mono">{lotsText(p)}</TableCell>
          <TableCell className="text-xs">{p.product}</TableCell>
          <TableCell className="text-right font-mono">
            <PriceAt value={p.average_price} at={p.entry_time} label="Entry" />
          </TableCell>
          <TableCell className="text-right font-mono">{price(p.ltp)}</TableCell>
          <TableCell />
          <TableCell className={`text-right font-mono ${pnlClass(p.mtm)}`}>{inr(p.mtm)}</TableCell>
        </TableRow>
      ))}
    </>
  )
}

/** Kite-style net positions: one row per contract, no strategy names. */
export function NetPositionsTable({ rows, total }: { rows: NetPosition[]; total: number }) {
  const ordered = [...rows.filter((r) => r.quantity !== 0), ...rows.filter((r) => r.quantity === 0)]
  if (ordered.length === 0) return <p className="text-sm text-muted-foreground">No positions.</p>
  return (
    <div className="overflow-x-auto">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Product</TableHead>
            <TableHead>Instrument</TableHead>
            <TableHead className="text-right">Qty.</TableHead>
            <TableHead className="text-right">Avg.</TableHead>
            <TableHead className="text-right">LTP</TableHead>
            <TableHead className="text-right">P&amp;L</TableHead>
            <TableHead className="text-right">Chg.</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {ordered.map((r) => {
            const flat = r.quantity === 0
            return (
              <TableRow
                key={`${r.symbol}-${r.exchange}-${r.product}`}
                className={flat ? 'text-muted-foreground' : ''}
                data-testid="net-row"
              >
                <TableCell>
                  <Badge variant="outline" className="text-xs">
                    {r.product}
                  </Badge>
                </TableCell>
                <TableCell>
                  <span className="font-mono text-sm">{r.symbol}</span>{' '}
                  <span className="text-xs text-muted-foreground">{r.exchange}</span>
                  {r.mismatch && (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <span className="ml-2 cursor-help text-xs text-amber-600">⚠ broker</span>
                      </TooltipTrigger>
                      <TooltipContent>
                        Broker shows {r.broker_quantity ?? 0} for this contract; the strategies hold{' '}
                        {r.quantity}.
                      </TooltipContent>
                    </Tooltip>
                  )}
                </TableCell>
                <TableCell
                  className={`text-right font-mono ${
                    flat ? '' : sideTextClass(r.quantity, r.symbol, r.instrument_type)
                  }`}
                >
                  {r.quantity > 0 ? `+${r.quantity}` : r.quantity}
                </TableCell>
                <TableCell className="text-right font-mono">
                  {flat ? (
                    <PriceAt value={r.average_price} at={r.exit_time} label="Exit" />
                  ) : (
                    <PriceAt value={r.average_price} at={r.entry_time} label="Entry" />
                  )}
                </TableCell>
                <TableCell className="text-right font-mono">{price(r.ltp)}</TableCell>
                <TableCell className={`text-right font-mono ${pnlClass(r.pnl)}`}>
                  {inr(r.pnl)}
                </TableCell>
                <TableCell className={`text-right font-mono text-xs ${pnlClass(r.change_percent)}`}>
                  {r.change_percent == null ? '' : `${r.change_percent.toFixed(2)}%`}
                </TableCell>
              </TableRow>
            )
          })}
          <TableRow>
            <TableCell colSpan={5} className="text-right font-medium">
              Total P&amp;L
            </TableCell>
            <TableCell
              className={`text-right font-mono font-medium ${pnlClass(total)}`}
              data-testid="net-total"
            >
              {inr(total)}
            </TableCell>
            <TableCell />
          </TableRow>
        </TableBody>
      </Table>
    </div>
  )
}

const VIEW_KEY = 'bridge.strategyPositions.view'

const MODE_KEY = 'bridge.strategyPositions.mode'

function storedMode(): DeskMode | null {
  try {
    const v = localStorage.getItem(MODE_KEY)
    return v === 'live' || v === 'sandbox' ? v : null
  } catch {
    return null
  }
}

/** One switch: Live on shows the live book, off the strategies' own book. */
function LiveSwitch({ mode, onChange }: { mode: DeskMode; onChange: (m: DeskMode) => void }) {
  return (
    <div className="flex items-center gap-2">
      <Switch
        id="strategy-positions-live"
        checked={mode === 'live'}
        onCheckedChange={(on) => onChange(on ? 'live' : 'sandbox')}
      />
      <Label htmlFor="strategy-positions-live" className="text-sm font-medium">
        Live
      </Label>
    </div>
  )
}

export default function StrategyDesk() {
  const [chosen, setChosen] = useState<DeskMode | null>(storedMode)
  const [defaultMode, setDefaultMode] = useState<DeskMode>('sandbox')
  const mode: DeskMode = chosen ?? defaultMode
  const choose = (m: DeskMode) => {
    setChosen(m)
    try {
      localStorage.setItem(MODE_KEY, m)
    } catch {
      /* not remembered */
    }
  }
  const [showClosed, setShowClosed] = useState(true)
  const [view, setView] = useState<'strategy' | 'net'>(() => {
    try {
      return localStorage.getItem(VIEW_KEY) === 'net' ? 'net' : 'strategy'
    } catch {
      return 'strategy'
    }
  })
  const chooseView = (v: 'strategy' | 'net') => {
    setView(v)
    try {
      localStorage.setItem(VIEW_KEY, v)
    } catch {
      /* not remembered */
    }
  }
  const desk = useQuery({
    queryKey: ['strategy-desk', mode],
    queryFn: () => fetchDesk(mode),
    refetchInterval: 3000,
  })
  const margin = useQuery({
    queryKey: ['strategy-desk-margin', mode],
    queryFn: () => fetchDeskMargin(mode),
    refetchInterval: 30000,
    retry: false,
  })
  const data = desk.data
  // With no remembered choice: Live when any strategy may trade live.
  useEffect(() => {
    if (chosen === null && data?.any_live_enabled) setDefaultMode('live')
  }, [chosen, data?.any_live_enabled])
  const qc = useQueryClient()
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ['strategy-desk'] })
    qc.invalidateQueries({ queryKey: ['strategy-desk-margin'] })
  }
  const open = data?.strategies.filter((s) => s.positions.length > 0) ?? []
  // Both books, for the Close all dialog's counts.
  const other = useQuery({
    queryKey: ['strategy-desk', mode === 'live' ? 'sandbox' : 'live'],
    queryFn: () => fetchDesk(mode === 'live' ? 'sandbox' : 'live'),
    refetchInterval: 10000,
    retry: false,
  })
  const counts = (() => {
    if (!data) return null
    const all = [...data.strategies, ...(other.data?.strategies ?? [])].filter(
      (s) => s.status === 'running' && s.positions.length > 0
    )
    return {
      strategies: new Set(all.map((s) => s.id)).size,
      positions: all.reduce((n, s) => n + s.positions.length, 0),
    }
  })()
  const openCount = open.reduce((n, s) => n + s.positions.length, 0)
  const closed = data?.strategies.flatMap((s) => s.closed_today.map((t) => ({ s, t }))) ?? []
  const m = margin.data

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-2xl font-bold tracking-tight">Strategy Positions</h1>
          <p className="text-muted-foreground">
            Open positions and today's P&amp;L of every strategy
          </p>
        </div>
        <div className="flex items-center gap-4">
          <CloseAllButton counts={counts} onDone={refresh} />
          <LiveSwitch mode={mode} onChange={choose} />
        </div>
      </div>

      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
        <Stat
          label="Total P&L"
          value={inr(data?.totals.total)}
          cls={pnlClass(data?.totals.total)}
        />
        <Stat
          label="Realized"
          value={inr(data?.totals.realized)}
          cls={pnlClass(data?.totals.realized)}
        />
        <Stat
          label="Unrealized"
          value={inr(data?.totals.unrealized)}
          cls={pnlClass(data?.totals.unrealized)}
        />
        <Stat
          label="Total margin used"
          value={m?.available ? inr(m.combined) : '—'}
          sub={marginSub(m, margin.isFetching)}
          info="Margin Zerodha would block for these positions, hedges combined"
        />
        <Stat label="Open positions" value={data ? String(openCount) : '—'} />
      </div>

      {desk.isError && (
        <p className="text-sm text-muted-foreground">Could not read the positions right now.</p>
      )}
      {data && !data.book_available && (
        <p className="text-xs text-muted-foreground">
          LTP unavailable: showing positions from the strategies' own fills.
        </p>
      )}

      <div className="inline-flex rounded-md border p-0.5" role="tablist" aria-label="View">
        {(
          [
            ['strategy', 'By strategy'],
            ['net', 'Net'],
          ] as const
        ).map(([v, label]) => (
          <button
            key={v}
            type="button"
            role="tab"
            aria-selected={view === v}
            onClick={() => chooseView(v)}
            className={`rounded px-3 py-1 text-sm ${
              view === v ? 'bg-primary text-primary-foreground' : 'text-muted-foreground'
            }`}
          >
            {label}
          </button>
        ))}
      </div>

      {view === 'net' ? (
        <Card>
          <CardHeader>
            <CardTitle>Positions ({data?.net?.length ?? 0})</CardTitle>
          </CardHeader>
          <CardContent>
            <NetPositionsTable rows={data?.net ?? []} total={data?.net_total ?? 0} />
          </CardContent>
        </Card>
      ) : (
        <>
          <Card>
            <CardHeader>
              <CardTitle>Open positions</CardTitle>
              <CardDescription>Grouped by strategy</CardDescription>
            </CardHeader>
            <CardContent>
              {open.length === 0 ? (
                <p className="text-sm text-muted-foreground">
                  {desk.isLoading ? 'Loading…' : 'No open positions.'}
                </p>
              ) : (
                <div className="overflow-x-auto">
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>Leg</TableHead>
                        <TableHead>Symbol</TableHead>
                        <TableHead>Side</TableHead>
                        <TableHead className="text-right">Qty (lots/units)</TableHead>
                        <TableHead>Product</TableHead>
                        <TableHead className="text-right">Avg entry</TableHead>
                        <TableHead className="text-right">LTP</TableHead>
                        <TableHead />
                        <TableHead className="text-right">MTM</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {open.map((s) => (
                        <StrategyGroup
                          key={s.id}
                          s={s}
                          margin={m?.per_strategy?.[String(s.id)]}
                          onChanged={refresh}
                        />
                      ))}
                    </TableBody>
                  </Table>
                </div>
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <button
                type="button"
                className="flex items-center gap-2 text-left"
                onClick={() => setShowClosed((v) => !v)}
                aria-expanded={showClosed}
              >
                {showClosed ? (
                  <ChevronDown className="h-4 w-4" />
                ) : (
                  <ChevronRight className="h-4 w-4" />
                )}
                <CardTitle>Closed today ({closed.length})</CardTitle>
              </button>
            </CardHeader>
            {showClosed && (
              <CardContent>
                {closed.length === 0 ? (
                  <p className="text-sm text-muted-foreground">No trades closed today.</p>
                ) : (
                  <div className="overflow-x-auto">
                    <Table>
                      <TableHeader>
                        <TableRow>
                          <TableHead>Strategy</TableHead>
                          <TableHead>Symbol</TableHead>
                          <TableHead>Side</TableHead>
                          <TableHead className="text-right">Qty</TableHead>
                          <TableHead className="text-right">Entry</TableHead>
                          <TableHead className="text-right">Exit</TableHead>
                          <TableHead>Exit reason</TableHead>
                          <TableHead className="text-right">P&amp;L</TableHead>
                        </TableRow>
                      </TableHeader>
                      <TableBody>
                        {closed.map(({ s, t }) => (
                          <TableRow key={`${s.id}-${t.run_id}-${t.leg_id}-${t.exit_time}`}>
                            <TableCell>
                              <Link to={`/strategy/${s.id}`} className="hover:underline">
                                {s.name}
                              </Link>
                            </TableCell>
                            <TableCell className="font-mono text-sm">{t.symbol}</TableCell>
                            <TableCell className={sideTextClass(t.side, t.symbol)}>
                              {t.side === 'long' ? 'Long' : 'Short'}
                            </TableCell>
                            <TableCell className="text-right font-mono">{t.qty}</TableCell>
                            <TableCell className="text-right font-mono">
                              <PriceAt value={t.entry_price} at={t.entry_time} label="Entry" />
                            </TableCell>
                            <TableCell className="text-right font-mono">
                              <PriceAt value={t.exit_price} at={t.exit_time} label="Exit" />
                            </TableCell>
                            <TableCell className="text-xs">{t.exit_kind}</TableCell>
                            <TableCell className={`text-right font-mono ${pnlClass(t.pnl)}`}>
                              {inr(t.pnl)}
                            </TableCell>
                          </TableRow>
                        ))}
                      </TableBody>
                    </Table>
                  </div>
                )}
              </CardContent>
            )}
          </Card>
        </>
      )}
    </div>
  )
}

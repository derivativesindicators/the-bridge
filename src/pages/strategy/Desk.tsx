// pages/strategy/Desk.tsx
// The Bridge 1.0.10: the Strategy desk. Every open position of every strategy
// in one book (Live switch on: live, off: the strategy sandbox), live LTP and MTM, per-strategy subtotals,
// total day P&L, the margin the positions block, and today's closed trades.
// Strategy-module trades live in their own book, so they are not on the
// Orderbook, Tradebook or Positions pages.

import { useQuery } from '@tanstack/react-query'
import { ChevronDown, ChevronRight, Info } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Link } from 'react-router'
import {
  contractText,
  type DeskMargin,
  type DeskMode,
  type DeskStrategy,
  fetchDesk,
  fetchDeskMargin,
  lotsText,
} from '@/api/desk'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
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

const inr = (v: number | null | undefined) =>
  v == null || !Number.isFinite(v)
    ? '—'
    : v.toLocaleString('en-IN', { style: 'currency', currency: 'INR', maximumFractionDigits: 2 })
const price = (v: number | null | undefined) =>
  v == null || !Number.isFinite(v) ? '—' : v.toFixed(2)
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

function StrategyGroup({ s, margin }: { s: DeskStrategy; margin: number | undefined }) {
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
            {p.source === 'broker/shared' && (
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
            <Badge variant={p.side === 'long' ? 'default' : 'destructive'} className="text-xs">
              {p.side === 'long' ? 'Long' : 'Short'}
            </Badge>
          </TableCell>
          <TableCell className="text-right font-mono">{lotsText(p)}</TableCell>
          <TableCell className="text-xs">{p.product}</TableCell>
          <TableCell className="text-right font-mono">{price(p.average_price)}</TableCell>
          <TableCell className="text-right font-mono">{price(p.ltp)}</TableCell>
          <TableCell />
          <TableCell className={`text-right font-mono ${pnlClass(p.mtm)}`}>{inr(p.mtm)}</TableCell>
        </TableRow>
      ))}
    </>
  )
}

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
  const open = data?.strategies.filter((s) => s.positions.length > 0) ?? []
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
        <LiveSwitch mode={mode} onChange={choose} />
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
                    <StrategyGroup key={s.id} s={s} margin={m?.per_strategy?.[String(s.id)]} />
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
                        <TableCell>{t.side === 'long' ? 'Long' : 'Short'}</TableCell>
                        <TableCell className="text-right font-mono">{t.qty}</TableCell>
                        <TableCell className="text-right font-mono">
                          {price(t.entry_price)}
                        </TableCell>
                        <TableCell className="text-right font-mono">
                          {price(t.exit_price)}
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
    </div>
  )
}

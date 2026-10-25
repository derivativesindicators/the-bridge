/**
 * The Bridge 1.0.6: symbol search across exchanges for the alert builders.
 * Upstream searched only the Exchange picked in the dropdown (default NSE),
 * so indices (NSE_INDEX) and their futures/options (NFO) never appeared.
 */

export interface SymbolHit {
  symbol: string
  name: string
  exchange: string
  token: string
  instrumenttype?: string
  expiry?: string
}

/** Searched in this order; the list is limited to what the broker supports. */
const PREFERRED = [
  'NSE',
  'NSE_INDEX',
  'NFO',
  'BSE',
  'BSE_INDEX',
  'BFO',
  'MCX',
  'CDS',
  'BCD',
  'NCO',
  'CRYPTO',
]

export function isIndexExchange(exchange: string): boolean {
  return exchange.toUpperCase().endsWith('_INDEX')
}

/** Exchanges to search: preferred ones the broker supports, index venues included. */
export function searchExchanges(supported: string[]): string[] {
  const have = new Set(supported.map((e) => e.toUpperCase()))
  // Index venues are not always listed as broker capabilities; their
  // cash/F&O parents are, so include the index next to its parent.
  if (have.has('NSE') || have.has('NFO')) have.add('NSE_INDEX')
  if (have.has('BSE') || have.has('BFO')) have.add('BSE_INDEX')
  const out = PREFERRED.filter((e) => have.has(e))
  for (const e of have) if (!out.includes(e)) out.push(e)
  return out
}

const kind = (h: SymbolHit): number => {
  if (isIndexExchange(h.exchange)) return 1
  const s = h.symbol.toUpperCase()
  if (s.endsWith('FUT')) return 2
  if (s.endsWith('CE') || s.endsWith('PE')) return 3
  return 0
}

/**
 * Ranks hits so the obvious pick is first: exact symbol, then symbols that
 * start with the query, then by kind (cash, index, future, option) and
 * nearest expiry. Keeps a few of each kind so options cannot crowd out the
 * index and futures.
 */
export function rankHits(hits: SymbolHit[], query: string, limit = 15): SymbolHit[] {
  const q = query.trim().toUpperCase()
  const time = (h: SymbolHit) => {
    const t = h.expiry ? Date.parse(h.expiry.replace(/-/g, ' ')) : Number.NaN
    return Number.isNaN(t) ? 0 : t
  }
  const sorted = [...hits].sort((a, b) => {
    const as = a.symbol.toUpperCase()
    const bs = b.symbol.toUpperCase()
    const ae = as === q ? 0 : as.startsWith(q) ? 1 : 2
    const be = bs === q ? 0 : bs.startsWith(q) ? 1 : 2
    if (ae !== be) return ae - be
    if (kind(a) !== kind(b)) return kind(a) - kind(b)
    if (time(a) !== time(b)) return time(a) - time(b)
    return as.localeCompare(bs)
  })
  const perKind = new Map<number, number>()
  const out: SymbolHit[] = []
  const rest: SymbolHit[] = []
  for (const h of sorted) {
    const k = kind(h)
    const n = perKind.get(k) ?? 0
    if (n < 4 || h.symbol.toUpperCase() === q) {
      perKind.set(k, n + 1)
      out.push(h)
    } else rest.push(h)
  }
  return [...out, ...rest].slice(0, limit)
}

// The Bridge 1.0.11: colour a side by market direction, not by buy/sell.
// A long CE or a short PE profits when the market rises (green); a short CE
// or a long PE profits when it falls (red). Non-options keep Long/Buy green
// and Short/Sell red. Only the colour changes; the label text stays.

export type OptionType = 'CE' | 'PE' | null

/** CE / PE from the instrument type when known, else from the symbol suffix. */
export function optionTypeOf(symbol?: string | null, instrumentType?: string | null): OptionType {
  const t = (instrumentType ?? '').toUpperCase()
  if (t === 'CE' || t === 'PE') return t
  const s = (symbol ?? '').trim().toUpperCase()
  if (/\d(CE)$/.test(s)) return 'CE'
  if (/\d(PE)$/.test(s)) return 'PE'
  return null
}

/** Whether the side is long/buy (anything else is short/sell). */
function isLongSide(side: string | number | null | undefined): boolean {
  if (typeof side === 'number') return side > 0
  const s = String(side ?? '')
    .trim()
    .toUpperCase()
  return s === 'LONG' || s === 'BUY' || s === 'B'
}

/** True when the position gains as the market rises (shown green). */
export function isBullish(
  side: string | number | null | undefined,
  symbol?: string | null,
  instrumentType?: string | null
): boolean {
  const long = isLongSide(side)
  return optionTypeOf(symbol, instrumentType) === 'PE' ? !long : long
}

/** Badge classes (solid background). */
export function sideBadgeClass(
  side: string | number | null | undefined,
  symbol?: string | null,
  instrumentType?: string | null
): string {
  return isBullish(side, symbol, instrumentType)
    ? 'bg-green-600 text-white hover:bg-green-600 border-transparent'
    : 'bg-red-600 text-white hover:bg-red-600 border-transparent'
}

/** Text classes. */
export function sideTextClass(
  side: string | number | null | undefined,
  symbol?: string | null,
  instrumentType?: string | null
): string {
  return isBullish(side, symbol, instrumentType)
    ? 'text-green-600 dark:text-green-400'
    : 'text-red-600 dark:text-red-400'
}

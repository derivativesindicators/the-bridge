// The Bridge 1.0.12: a fill time in IST beside an entry or exit price.
// Today's fills show the time only ("13:45:14 IST"); an earlier day's (an
// overnight carry) shows the date too ("26 Oct 13:45:14 IST").

/** Parse a stored UTC stamp ("2026-10-26 08:15:14.004171", no zone) or an ISO one. */
export function parseUtcStamp(ts: string | null | undefined): Date | null {
  if (!ts) return null
  let s = ts.trim().replace(' ', 'T')
  // Trim sub-millisecond digits, which Date does not read everywhere.
  s = s.replace(/(\.\d{3})\d+/, '$1')
  if (!/(Z|[+-]\d{2}:?\d{2})$/.test(s)) s += 'Z'
  const d = new Date(s)
  return Number.isNaN(d.getTime()) ? null : d
}

const dayKey = (d: Date) => d.toLocaleDateString('en-CA', { timeZone: 'Asia/Kolkata' })

export function istStamp(ts: string | null | undefined, now: Date = new Date()): string | null {
  const d = parseUtcStamp(ts)
  if (!d) return null
  const time = d.toLocaleTimeString('en-GB', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
    timeZone: 'Asia/Kolkata',
  })
  if (dayKey(d) === dayKey(now)) return `${time} IST`
  const date = d.toLocaleDateString('en-GB', {
    day: '2-digit',
    month: 'short',
    timeZone: 'Asia/Kolkata',
  })
  return `${date} ${time} IST`
}

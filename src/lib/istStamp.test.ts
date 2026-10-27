import { describe, expect, it } from 'vitest'
import { istStamp, parseUtcStamp } from './istStamp'

const now = new Date('2026-10-27T07:00:00Z') // 12:30 IST, 27 Oct

describe('istStamp', () => {
  it('shows only the time for a fill today (IST)', () => {
    expect(istStamp('2026-10-27 05:50:10.278527', now)).toBe('11:20:10 IST')
  })
  it('shows the date for an overnight carry', () => {
    expect(istStamp('2026-10-26 08:15:14.004171', now)).toBe('26 Oct 13:45:14 IST')
  })
  it('uses the IST day, not the UTC day', () => {
    // 26 Oct 19:00 UTC is 27 Oct 00:30 IST: today.
    expect(istStamp('2026-10-26 19:00:00', now)).toBe('00:30:00 IST')
  })
  it('reads ISO stamps with an offset', () => {
    expect(istStamp('2026-10-27T05:50:10+00:00', now)).toBe('11:20:10 IST')
  })
  it('is null for nothing or junk', () => {
    expect(istStamp(null, now)).toBeNull()
    expect(istStamp('junk', now)).toBeNull()
    expect(parseUtcStamp('')).toBeNull()
  })
})

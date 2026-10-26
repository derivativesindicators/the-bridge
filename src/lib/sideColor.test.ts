import { describe, expect, it } from 'vitest'
import { isBullish, optionTypeOf, sideBadgeClass, sideTextClass } from './sideColor'

describe('side colour by market direction', () => {
  it('reads the option type from the symbol or instrument type', () => {
    expect(optionTypeOf('NIFTY27OCT2622700PE')).toBe('PE')
    expect(optionTypeOf('NIFTY27OCT2622700CE')).toBe('CE')
    expect(optionTypeOf('NIFTY27OCT26FUT')).toBeNull()
    expect(optionTypeOf('RELIANCE')).toBeNull()
    expect(optionTypeOf('X', 'pe')).toBe('PE')
  })
  it('CE long green, CE short red, PE long red, PE short green', () => {
    expect(isBullish('long', 'NIFTY27OCT2623000CE')).toBe(true)
    expect(isBullish('short', 'NIFTY27OCT2623000CE')).toBe(false)
    expect(isBullish('BUY', 'NIFTY27OCT2623000PE')).toBe(false)
    expect(isBullish('SELL', 'NIFTY27OCT2623000PE')).toBe(true)
    expect(sideBadgeClass('short', 'NIFTY27OCT2622700PE')).toContain('green')
    expect(sideTextClass('long', 'NIFTY27OCT2622700PE')).toContain('red')
  })
  it('non-options keep long green and short red', () => {
    expect(isBullish('long', 'NIFTY27OCT26FUT')).toBe(true)
    expect(isBullish('SELL', 'RELIANCE')).toBe(false)
    expect(isBullish(-75, 'RELIANCE')).toBe(false)
  })
})

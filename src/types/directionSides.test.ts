import { describe, expect, it } from 'vitest'
import { directionSidesWithoutLeg, directionSidesWithoutLegError } from './strategy_module'

describe('a direction needs a leg on each side it listens to', () => {
  it('flags Long+Short with only long legs', () => {
    const legs = [{ side: 'long' as const }, { side: 'long' as const }]
    expect(directionSidesWithoutLeg('both', legs)).toEqual(['short'])
    expect(directionSidesWithoutLegError('both', legs)).toMatch(/no leg on the Short side/)
    expect(directionSidesWithoutLegError('both', legs)).toMatch(/SELL\/short_entry/)
  })
  it('accepts one leg per side, or a both-sided leg', () => {
    expect(directionSidesWithoutLeg('both', [{ side: 'long' }, { side: 'short' }])).toEqual([])
    expect(directionSidesWithoutLeg('both', [{ side: 'both' }])).toEqual([])
    expect(directionSidesWithoutLeg('both', [{}])).toEqual([])
  })
  it('checks only the listened side for long-only and short-only', () => {
    expect(directionSidesWithoutLeg('long_only', [{ side: 'long' }])).toEqual([])
    expect(directionSidesWithoutLeg('short_only', [{ side: 'long' }])).toEqual(['short'])
    expect(directionSidesWithoutLeg('long_only', [])).toEqual(['long'])
    expect(directionSidesWithoutLegError('short_only', [{ side: 'short' }])).toBeNull()
  })
})

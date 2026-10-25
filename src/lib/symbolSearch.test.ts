import { describe, expect, it } from 'vitest'
import { isIndexExchange, rankHits, searchExchanges } from './symbolSearch'

const h = (symbol: string, exchange: string, expiry = '') => ({
  symbol,
  exchange,
  name: '',
  token: symbol,
  expiry,
})

describe('symbolSearch', () => {
  it('searches index and F&O venues, not only the dropdown exchange', () => {
    expect(searchExchanges(['NSE', 'NFO', 'BSE', 'BFO', 'MCX'])).toEqual([
      'NSE',
      'NSE_INDEX',
      'NFO',
      'BSE',
      'BSE_INDEX',
      'BFO',
      'MCX',
    ])
  })

  it('puts the index and its futures before a wall of options', () => {
    const opts = Array.from({ length: 30 }, (_, i) => h(`NIFTY28OCT25${24000 + i * 50}CE`, 'NFO'))
    const r = rankHits(
      [
        ...opts,
        h('NIFTY28OCT25FUT', 'NFO', '28-OCT-25'),
        h('NIFTY', 'NSE_INDEX'),
        h('NIFTYBEES', 'NSE'),
      ],
      'NIFTY'
    )
    expect(r[0]).toMatchObject({ symbol: 'NIFTY', exchange: 'NSE_INDEX' })
    expect(r.map((x) => x.symbol)).toContain('NIFTY28OCT25FUT')
    expect(r.map((x) => x.symbol)).toContain('NIFTYBEES')
    expect(r.length).toBe(15)
  })

  it('knows index exchanges', () => {
    expect(isIndexExchange('NSE_INDEX')).toBe(true)
    expect(isIndexExchange('NFO')).toBe(false)
  })
})

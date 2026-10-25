import { fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'
import { describe, expect, it } from 'vitest'
import {
  DEFAULT_SIGNAL_OPTIONS,
  type SignalOptions,
  signalOptionsError,
  signalOptionsFrom,
  signalOptionsPayload,
} from '@/types/strategy_module'
import { SignalOptionsCard } from './SignalOptionsCard'

describe('signal options helpers', () => {
  it('all off sends null, and batch strategies never send options', () => {
    expect(signalOptionsPayload(DEFAULT_SIGNAL_OPTIONS, 'signal')).toBeNull()
    const on = { ...DEFAULT_SIGNAL_OPTIONS, cas_exit: true }
    expect(signalOptionsPayload(on, 'batch')).toBeNull()
    expect(signalOptionsPayload(on, 'signal')).toMatchObject({
      cas_exit: true,
      session_end: '15:40',
      cas_start: '15:15',
      session_times: 'auto',
    })
  })
  it('merges stored values onto the defaults', () => {
    expect(signalOptionsFrom({ expiry_roll: true }).roll_time).toBe('15:25')
    expect(signalOptionsFrom(null)).toEqual(DEFAULT_SIGNAL_OPTIONS)
  })
  it('mirrors the server checks', () => {
    const carry = { ...DEFAULT_SIGNAL_OPTIONS, carry_overnight: true }
    expect(signalOptionsError(carry, 'intraday', 'NRML')).toMatch(/positional/)
    expect(signalOptionsError(carry, 'positional', 'MIS')).toMatch(/NRML/)
    expect(signalOptionsError(carry, 'positional', 'NRML')).toBeNull()
    const roll = {
      ...DEFAULT_SIGNAL_OPTIONS,
      expiry_roll: true,
      reopen: 'immediate' as const,
      roll_time: '15:30',
    }
    expect(signalOptionsError(roll, 'intraday', 'MIS')).toMatch(/before 15:30/)
    expect(signalOptionsError({ ...roll, roll_time: '15:29' }, 'intraday', 'MIS')).toBeNull()
    expect(signalOptionsError({ ...roll, reopen: 'next_session' }, 'positional', 'NRML')).toMatch(
      /carry/
    )
    const cas = {
      ...DEFAULT_SIGNAL_OPTIONS,
      cas_exit: true,
      session_times: 'custom' as const,
      cas_start: '15:41',
    }
    expect(signalOptionsError(cas, 'intraday', 'MIS')).toMatch(/earlier/)
  })
})

function Harness({ onValue }: { onValue: (v: SignalOptions) => void }) {
  const [v, setV] = useState(DEFAULT_SIGNAL_OPTIONS)
  return (
    <SignalOptionsCard
      value={v}
      onChange={(n) => {
        setV(n)
        onValue(n)
      }}
    />
  )
}

describe('SignalOptionsCard', () => {
  it('starts all off and reveals the CAS and roll settings when ticked', () => {
    let last = DEFAULT_SIGNAL_OPTIONS
    render(<Harness onValue={(v) => (last = v)} />)
    expect(screen.queryByLabelText(/Timeframe/)).toBeNull()
    fireEvent.click(screen.getByLabelText(/Closing-session \(CAS\) exit/))
    expect(screen.getByText(/From 7 Dec\s+2026: 15:30 to 15:45/)).toBeTruthy()
    expect(screen.queryByLabelText(/F&O session end/)).toBeNull()
    fireEvent.change(screen.getByLabelText('Session times'), { target: { value: 'custom' } })
    fireEvent.change(screen.getByLabelText(/F&O session end/), { target: { value: '15:39' } })
    expect(last).toMatchObject({ cas_exit: true, session_times: 'custom', session_end: '15:39' })
    fireEvent.click(screen.getByLabelText(/Roll on expiry day/))
    fireEvent.change(screen.getByLabelText('Reopen'), { target: { value: 'immediate' } })
    expect(screen.queryByLabelText('Reopen time')).toBeNull()
    expect(last).toMatchObject({ expiry_roll: true, reopen: 'immediate' })
  })
})

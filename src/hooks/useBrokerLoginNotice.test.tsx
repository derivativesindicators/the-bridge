import { render } from '@testing-library/react'
import { MemoryRouter, useLocation } from 'react-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const success = vi.fn()
vi.mock('sonner', () => ({ toast: { success: (m: string) => success(m) } }))

import { brokerLabel, useBrokerLoginNotice } from './useBrokerLoginNotice'

let search = ''
function Probe() {
  useBrokerLoginNotice()
  search = useLocation().search
  return null
}

describe('useBrokerLoginNotice', () => {
  beforeEach(() => success.mockReset())

  it('toasts once and clears the flag', () => {
    render(
      <MemoryRouter initialEntries={['/dashboard?broker_login=success&broker=zerodha']}>
        <Probe />
      </MemoryRouter>
    )
    expect(success).toHaveBeenCalledTimes(1)
    expect(success).toHaveBeenCalledWith('Zerodha login successful')
    expect(search).toBe('')
  })

  it('does nothing without the flag', () => {
    render(
      <MemoryRouter initialEntries={['/dashboard']}>
        <Probe />
      </MemoryRouter>
    )
    expect(success).not.toHaveBeenCalled()
  })

  it('labels brokers', () => {
    expect(brokerLabel('zerodha')).toBe('Zerodha')
    expect(brokerLabel('')).toBe('Broker')
  })
})

import { renderHook, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { useFeedServerProblem } from './useFeedServerProblem'

const MSG =
  'Live market data ... could not start because port 8770 is already used by another program, most likely OpenAlgo Desktop or another copy of The Bridge.'

afterEach(() => vi.unstubAllGlobals())

function stub(ws: unknown) {
  const f = vi.fn(async () => ({
    json: async () => ({ status: 'success', data: { ws_status: ws } }),
  }))
  vi.stubGlobal('fetch', f)
  return f
}

describe('useFeedServerProblem', () => {
  it('names the taken port and the likely other app', async () => {
    stub({ state: 'port_in_use', port: 8770, message: MSG })
    const { result } = renderHook(() => useFeedServerProblem(true))
    await waitFor(() => expect(result.current).toBe(MSG))
    expect(result.current).toContain('OpenAlgo Desktop')
  })
  it('says nothing while the listener runs', async () => {
    const f = stub({ state: 'running', port: 8770, message: null })
    const { result } = renderHook(() => useFeedServerProblem(true))
    await waitFor(() => expect(f).toHaveBeenCalled())
    expect(result.current).toBeNull()
  })
  it('does not ask while the feed is live', () => {
    const f = stub({ state: 'running' })
    renderHook(() => useFeedServerProblem(false))
    expect(f).not.toHaveBeenCalled()
  })
})

import { afterEach, describe, expect, it, vi } from 'vitest'
import { MarketDataManager } from './MarketDataManager'

/**
 * The Bridge 1.0.3: the feed server ends a socket *cleanly* (4401 auth
 * timeout / authentication expired, 1001 restart). The manager used to treat
 * any clean close as final, so pages stayed "Disconnected" while REST quotes
 * kept prices moving. A server close must reconnect; a pause must not.
 */
class FakeSocket {
  static instances: FakeSocket[] = []
  static OPEN = 1
  static CONNECTING = 0
  readyState = 0
  onopen: (() => void) | null = null
  onclose: ((e: { wasClean: boolean; code: number }) => void) | null = null
  onerror: (() => void) | null = null
  onmessage: ((e: MessageEvent) => void) | null = null
  sent: string[] = []
  constructor(public url: string) {
    FakeSocket.instances.push(this)
  }
  send(m: string) {
    this.sent.push(m)
  }
  close() {
    this.readyState = 3
  }
}

function stubNet() {
  FakeSocket.instances = []
  vi.stubGlobal('WebSocket', FakeSocket)
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => ({
      json: async () =>
        url.includes('csrf')
          ? { csrf_token: 't' }
          : url.includes('config')
            ? { status: 'success', websocket_url: 'ws://127.0.0.1:8765' }
            : { status: 'success', api_key: 'k' },
    }))
  )
}

const flush = () => new Promise((r) => setTimeout(r, 0))

afterEach(() => {
  MarketDataManager.resetInstance()
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('MarketDataManager reconnect', () => {
  it('reconnects after a clean server close (4401)', async () => {
    stubNet()
    const m = MarketDataManager.getInstance()
    await m.connect()
    expect(FakeSocket.instances).toHaveLength(1)
    FakeSocket.instances[0].onclose?.({ wasClean: true, code: 4401 })
    expect(m.getState().isConnected).toBe(false)
    await new Promise((r) => setTimeout(r, 1100))
    await flush()
    expect(FakeSocket.instances.length).toBe(2)
  })

  it('stays down after an explicit disconnect', async () => {
    stubNet()
    const m = MarketDataManager.getInstance()
    await m.connect()
    const s = FakeSocket.instances[0]
    m.disconnect()
    s.onclose?.({ wasClean: true, code: 1000 })
    await new Promise((r) => setTimeout(r, 1100))
    expect(FakeSocket.instances.length).toBe(1)
  })

  it('reports authenticated with no ticks at all (market closed)', async () => {
    stubNet()
    const m = MarketDataManager.getInstance()
    await m.connect()
    FakeSocket.instances[0].onmessage?.(
      new MessageEvent('message', { data: JSON.stringify({ type: 'auth', status: 'success' }) })
    )
    expect(m.getState().isAuthenticated).toBe(true)
    expect(m.getState().isConnected).toBe(true)
  })
})

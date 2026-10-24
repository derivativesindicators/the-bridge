import { describe, expect, it } from 'vitest'
import { render, screen, userEvent } from '@/test/test-utils'
import Faq from './Faq'

describe('FAQ system requirements', () => {
  it('says Raspberry Pi is not supported and points a Pi user to The Bridge web', async () => {
    render(<Faq />)
    await userEvent.click(screen.getByRole('button', { name: 'What are the system requirements?' }))
    const answer = await screen.findByText(/^The Bridge runs on Windows 10\/11/)
    expect(answer).toHaveTextContent('64-bit Linux (x64 or ARM64)')
    expect(answer).toHaveTextContent(
      'It does not support Raspberry Pi: run The Bridge web on your Pi instead.'
    )
  })
})

describe('FAQ security answer', () => {
  // NEW-SEC-03: the answer describes the same transport as Server Settings'
  // warning (plain http on this computer, unencrypted to other devices,
  // https only through a tunnel), not "HTTPS for all communications".
  it('says which links are encrypted and which are not', async () => {
    render(<Faq />)
    await userEvent.click(screen.getByRole('button', { name: 'How secure is The Bridge?' }))
    const answer = await screen.findByText(/^The Bridge keeps your broker credentials/)
    expect(answer).not.toHaveTextContent(/HTTPS for all communications/i)
    expect(answer).toHaveTextContent('plain http and ws on this computer (127.0.0.1)')
    expect(answer).toHaveTextContent("broker's own secure (https and wss) addresses")
    expect(answer).toHaveTextContent(
      'If you allow access from other devices in Server Settings, that traffic is not encrypted'
    )
    expect(answer).toHaveTextContent('reach The Bridge through a tunnel with https')
  })
})

import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { afterEach, describe, expect, it, vi } from 'vitest'
import About from './About'

describe('About & licence', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('carries the AGPL appropriate legal notices and the source links', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({ json: async () => ({ status: 'success', version: '1.0.11' }) })
    )
    render(<About />, { wrapper: MemoryRouter })
    expect(await screen.findByText('The Bridge v1.0.11')).toBeInTheDocument()
    expect(screen.getByText(/The Bridge is based on OpenAlgo/)).toBeInTheDocument()
    expect(screen.getByText(/modified by Derivatives Indicators/)).toBeInTheDocument()
    expect(screen.getByText('Original work Copyright 2024 OpenAlgo.')).toBeInTheDocument()
    expect(screen.getByText(/without warranty of any kind/)).toBeInTheDocument()
    expect(screen.getByText(/You may redistribute and\/or modify/)).toBeInTheDocument()
    const hrefs = screen.getAllByRole('link').map((a) => a.getAttribute('href'))
    expect(hrefs).toContain('https://github.com/derivativesindicators/the-bridge')
    expect(hrefs).toContain('https://github.com/marketcalls/openalgo')
    expect(hrefs).toContain('https://www.gnu.org/licenses/agpl-3.0.html')
    expect(screen.getByTestId('license-text').textContent).toContain(
      'GNU AFFERO GENERAL PUBLIC LICENSE'
    )
  })
})

import { fireEvent, render, screen } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { Layout } from '@/components/layout/Layout'
import { useAuthStore } from '@/stores/authStore'
import Download from './Download'

const openExternal = vi.fn(async (_url: string) => {})
vi.mock('@/lib/desktop', async (orig) => ({
  ...(await orig<typeof import('@/lib/desktop')>()),
  isDesktopShell: () => true,
  openExternal: (url: string) => openExternal(url),
}))

function at() {
  return render(
    <MemoryRouter initialEntries={['/download']}>
      <Routes>
        <Route element={<Layout allowPublic />}>
          <Route path="/download" element={<Download />} />
        </Route>
        <Route path="/login" element={<div>sign-in page</div>} />
      </Routes>
    </MemoryRouter>
  )
}

afterEach(() => {
  openExternal.mockClear()
  useAuthStore.setState({ user: null, isAuthenticated: false, apiKey: null })
})

describe('Download page (The Bridge 1.0.3)', () => {
  it('opens All releases and Source code in the system browser', () => {
    at()
    fireEvent.click(screen.getByRole('link', { name: /All releases/i }))
    expect(openExternal).toHaveBeenCalledWith(
      'https://github.com/derivativesindicators/the-bridge/releases'
    )
    fireEvent.click(screen.getAllByRole('link', { name: /Source code/i })[0])
    expect(openExternal).toHaveBeenCalledWith('https://github.com/derivativesindicators/the-bridge')
  })

  it('is reachable signed out with a way back, not bounced to sign-in', () => {
    at()
    expect(screen.queryByText('sign-in page')).not.toBeInTheDocument()
    expect(screen.getByRole('link', { name: /Back to Home/i })).toHaveAttribute('href', '/')
  })

  it('signed in: shows Back to Dashboard', () => {
    useAuthStore.setState({
      user: { username: 'a', broker: 'zerodha' } as never,
      isAuthenticated: true,
    })
    at()
    expect(screen.getByRole('link', { name: /Back to Dashboard/i })).toHaveAttribute(
      'href',
      '/dashboard'
    )
  })
})

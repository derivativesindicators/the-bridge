import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const tauri = vi.hoisted(() => ({
  inShell: false,
  invoke: vi.fn(async (_cmd: string, _args?: unknown): Promise<unknown> => undefined),
}))

vi.mock('@tauri-apps/api/core', () => ({
  isTauri: () => tauri.inShell,
  invoke: (cmd: string, args?: unknown) => tauri.invoke(cmd, args),
}))

import { passwordRules, StartFreshCard } from './StartFreshCard'

const GOOD = 'Fresh@1234'

async function typePasswords(password: string, confirm: string) {
  await userEvent.type(screen.getByLabelText('New password'), password)
  await userEvent.type(screen.getByLabelText('Confirm new password'), confirm)
}

describe('StartFreshCard (KEY-01)', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    tauri.inShell = false
  })

  it('says in trader words what stays and what is set up again', () => {
    render(<StartFreshCard reason="forgot" onDone={vi.fn()} />)
    expect(screen.getByRole('heading', { name: 'Set a new password' })).toBeInTheDocument()
    expect(screen.getByText(/keeps your account, settings, strategies/)).toBeInTheDocument()
    expect(screen.getByText(/enter your broker keys again/)).toBeInTheDocument()
    expect(screen.getByText(/copy your new API key/)).toBeInTheDocument()
    expect(screen.getByText(/set up two-factor login \(TOTP\) again/)).toBeInTheDocument()
  })

  it('in a browser tab explains where to go and offers no form', () => {
    render(<StartFreshCard reason="keychain" onDone={vi.fn()} />)
    expect(screen.getByRole('heading', { name: 'Start fresh' })).toBeInTheDocument()
    expect(screen.queryByLabelText('New password')).toBeNull()
    expect(screen.getByText(/only in the The Bridge window/)).toBeInTheDocument()
  })

  it('asks the desktop shell only once the new password meets the rules and matches', async () => {
    tauri.inShell = true
    tauri.invoke.mockResolvedValue({ status: 'done', message: 'Sign in with your new password.' })
    const onDone = vi.fn()
    render(<StartFreshCard reason="keychain" onDone={onDone} />)
    const button = screen.getByRole('button', { name: 'Start fresh' })
    expect(button).toBeDisabled()
    await typePasswords('weak', 'weak')
    expect(button).toBeDisabled()
    await userEvent.clear(screen.getByLabelText('New password'))
    await userEvent.clear(screen.getByLabelText('Confirm new password'))
    await typePasswords(GOOD, 'Other@1234')
    expect(screen.getByText('The passwords do not match.')).toBeInTheDocument()
    expect(button).toBeDisabled()
    await userEvent.clear(screen.getByLabelText('Confirm new password'))
    await userEvent.type(screen.getByLabelText('Confirm new password'), GOOD)
    expect(button).toBeEnabled()
    await userEvent.click(button)
    await waitFor(() => expect(onDone).toHaveBeenCalledWith('Sign in with your new password.'))
    expect(tauri.invoke).toHaveBeenCalledWith('start_fresh', {
      newPassword: GOOD,
      confirmPassword: GOOD,
      reason: 'keychain',
    })
  })

  it('changes nothing when the trader cancels the system confirmation', async () => {
    tauri.inShell = true
    tauri.invoke.mockResolvedValue({ status: 'cancelled', message: 'Nothing was changed.' })
    const onDone = vi.fn()
    render(<StartFreshCard reason="forgot" onDone={onDone} />)
    await typePasswords(GOOD, GOOD)
    await userEvent.click(screen.getByRole('button', { name: 'Set new password' }))
    await waitFor(() => expect(tauri.invoke).toHaveBeenCalled())
    expect(onDone).not.toHaveBeenCalled()
  })

  it('shows the shell refusal in trader words', async () => {
    tauri.inShell = true
    tauri.invoke.mockRejectedValue({
      code: 'AUTH_ERROR',
      message: 'A new password can be set only from the The Bridge window on this computer.',
    })
    render(<StartFreshCard reason="forgot" onDone={vi.fn()} />)
    await typePasswords(GOOD, GOOD)
    await userEvent.click(screen.getByRole('button', { name: 'Set new password' }))
    expect(await screen.findByRole('alert')).toHaveTextContent(/only from the The Bridge/)
  })

  it('checks the web password rules', () => {
    expect(passwordRules(GOOD).every((r) => r.met)).toBe(true)
    expect(passwordRules('fresh@1234').some((r) => !r.met)).toBe(true)
    expect(passwordRules('Fresh1234').some((r) => !r.met)).toBe(true)
  })
})

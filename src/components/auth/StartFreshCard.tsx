import { Check } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { isDesktopShell, type StartFreshReason, startFreshFromDesktop } from '@/lib/desktop'

/** The web's password rules (validate_password_strength), checked as typed. */
export function passwordRules(password: string) {
  return [
    { met: password.length >= 8, label: 'At least 8 characters' },
    { met: /[A-Z]/.test(password), label: 'One uppercase letter (A-Z)' },
    { met: /[a-z]/.test(password), label: 'One lowercase letter (a-z)' },
    { met: /[0-9]/.test(password), label: 'One number (0-9)' },
    { met: /[!@#$%^&*]/.test(password), label: 'One special character (!@#$%^&*)' },
  ]
}

interface StartFreshCardProps {
  reason: StartFreshReason
  /** Called once the new password is set, with the message to show. */
  onDone: (message: string) => void
}

/**
 * Desktop: keep the account, its settings and history, and set a new
 * password, when the old keys cannot be opened (a keychain of an earlier
 * build that cannot be read) or the password is forgotten. OpenAlgo locks
 * saved broker keys, the API key and the authenticator with the password,
 * so they cannot come back and are set up again. Works only inside the
 * OpenAlgo Desktop window, after a system confirmation.
 */
export function StartFreshCard({ reason, onDone }: StartFreshCardProps) {
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const inShell = isDesktopShell()
  const rules = passwordRules(password)
  const ready = rules.every((r) => r.met) && password === confirm
  const action = reason === 'forgot' ? 'Set new password' : 'Start fresh'

  const onSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!ready) return
    setBusy(true)
    setError('')
    try {
      const outcome = await startFreshFromDesktop(password, confirm, reason)
      if (outcome.status === 'done') {
        setPassword('')
        setConfirm('')
        onDone(outcome.message)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Your new password could not be set.')
    } finally {
      setBusy(false)
    }
  }

  return (
    <section
      aria-labelledby="start-fresh-heading"
      className="rounded-lg border p-4 space-y-3 text-sm"
    >
      <h2 id="start-fresh-heading" className="font-medium">
        {reason === 'forgot' ? 'Set a new password' : 'Start fresh'}
      </h2>
      <p className="text-muted-foreground">
        The Bridge locks your saved broker keys, your API key and your two-factor login with your
        password, so they cannot be opened without the old one. A new password keeps your account,
        settings, strategies, watchlists and trade history. After it you:
      </p>
      <ul className="list-disc pl-5 text-muted-foreground space-y-1">
        <li>enter your broker keys again on Broker Configuration and sign in to your broker;</li>
        <li>
          copy your new API key into every program that uses it (TradingView, Amibroker, Python
          scripts);
        </li>
        <li>set up two-factor login (TOTP) again in Profile, if you used it;</li>
        <li>add your Telegram bot again and pair WhatsApp again, if you used them.</li>
      </ul>
      {inShell ? (
        <form onSubmit={onSubmit} className="space-y-3">
          <div className="space-y-1">
            <Label htmlFor="fresh-password">New password</Label>
            <Input
              id="fresh-password"
              type="password"
              autoComplete="new-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              disabled={busy}
              required
            />
          </div>
          <div className="space-y-1">
            <Label htmlFor="fresh-confirm">Confirm new password</Label>
            <Input
              id="fresh-confirm"
              type="password"
              autoComplete="new-password"
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
              disabled={busy}
              required
            />
          </div>
          <ul className="space-y-1" aria-label="Password requirements">
            {rules.map((r) => (
              <li
                key={r.label}
                className={`flex items-center gap-2 ${r.met ? 'text-green-600' : 'text-muted-foreground'}`}
              >
                <Check className={`h-4 w-4 ${r.met ? 'opacity-100' : 'opacity-0'}`} />
                {r.label}
              </li>
            ))}
          </ul>
          {confirm !== '' && password !== confirm && (
            <p className="text-destructive">The passwords do not match.</p>
          )}
          <Button type="submit" className="w-full" disabled={busy || !ready}>
            {busy ? 'Waiting for your confirmation...' : action}
          </Button>
        </form>
      ) : (
        <p>
          For your safety this works only in the The Bridge window on the computer where it is
          installed. Open the app there and choose {action} on this page.
        </p>
      )}
      {error && (
        <p role="alert" className="text-destructive">
          {error}
        </p>
      )}
    </section>
  )
}

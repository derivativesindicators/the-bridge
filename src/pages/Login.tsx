import { ArrowLeft, Eye, EyeOff, KeyRound, Loader2, LogIn, ShieldCheck } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Link, useNavigate } from 'react-router'
// Desktop: the one-time move of an earlier build's keys (KEY-01).
import { StartFreshCard } from '@/components/auth/StartFreshCard'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { appWindowHeaders, type KeyRecovery, keyRecoveryOf } from '@/lib/desktop'
import { useAuthStore } from '@/stores/authStore'
import { showToast } from '@/utils/toast'

export default function Login() {
  const navigate = useNavigate()
  const { login: setLogin } = useAuthStore()
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [totpCode, setTotpCode] = useState('')
  // Desktop: 'keys' when the keys of an earlier build could not be read
  // (the server offers retrying the keychain and starting fresh), 'fresh'
  // for the start-fresh form.
  const [step, setStep] = useState<'password' | 'totp' | 'keys' | 'fresh'>('password')
  const [recovery, setRecovery] = useState<KeyRecovery[]>([])
  const [keysMessage, setKeysMessage] = useState('')
  const [notice, setNotice] = useState<string | null>(null)
  const [showPassword, setShowPassword] = useState(false)
  const [isLoading, setIsLoading] = useState(false)
  const [isCheckingSetup, setIsCheckingSetup] = useState(true)
  const [error, setError] = useState<string | null>(null)
  // Desktop: "First time? Set up your account" shows only when no account
  // exists yet (OpenBull's sign-in card).
  const [needsSetup, setNeedsSetup] = useState(false)

  // Check if setup is required or already logged in on page load
  useEffect(() => {
    const checkSetup = async () => {
      try {
        // First check if setup is needed
        const setupResponse = await fetch('/auth/check-setup', {
          credentials: 'include',
        })
        const setupData = await setupResponse.json()
        if (setupData.needs_setup) {
          setNeedsSetup(true)
          return
        }

        // Check if already logged in
        const sessionResponse = await fetch('/auth/session-status', {
          credentials: 'include',
        })

        // Only process if response is successful (not 401 etc.)
        if (sessionResponse.ok) {
          const sessionData = await sessionResponse.json()

          if (sessionData.status === 'success' && sessionData.logged_in && sessionData.broker) {
            // Already fully logged in with broker, go to dashboard
            navigate('/dashboard', { replace: true })
            return
          } else if (
            sessionData.status === 'success' &&
            sessionData.authenticated &&
            !sessionData.logged_in
          ) {
            // Logged in but no broker, go to broker selection
            navigate('/broker', { replace: true })
            return
          }
        }
        // If session check fails (401, etc.), just stay on login page
      } catch (_err) {
      } finally {
        setIsCheckingSetup(false)
      }
    }
    checkSetup()
  }, [navigate])

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    await submitPassword()
  }

  const submitPassword = async () => {
    setIsLoading(true)
    setError(null)
    setNotice(null)

    try {
      // First, fetch CSRF token
      const csrfResponse = await fetch('/auth/csrf-token', {
        credentials: 'include',
      })

      if (!csrfResponse.ok) {
        setError('Failed to initialize login. Please refresh the page.')
        setIsLoading(false)
        return
      }

      const csrfData = await csrfResponse.json()

      // Create form data with CSRF token (matches original Flask template approach)
      const formData = new FormData()
      formData.append('username', username)
      formData.append('password', password)
      formData.append('csrf_token', csrfData.csrf_token)

      // Use native fetch like the original template
      const response = await fetch('/auth/login', {
        method: 'POST',
        body: formData,
        credentials: 'include',
        // Desktop: the OpenAlgo window shows it is the window, so it alone
        // moves an earlier build's keys and hears why they could not be
        // opened (KEY-01). A browser tab sends nothing.
        headers: await appWindowHeaders(),
      })

      // Check content type before parsing
      const contentType = response.headers.get('content-type')
      if (!contentType || !contentType.includes('application/json')) {
        // If redirected to setup page, inform user
        if (response.url.includes('/setup')) {
          setError('Please complete initial setup first.')
          navigate('/setup')
        } else {
          setError('Login failed. Please try again.')
        }
        setIsLoading(false)
        return
      }

      const data = await response.json()

      // Desktop: the keys of an earlier build could not be read; the
      // server says which ways out there are.
      const keyRecovery = keyRecoveryOf(data)
      if (!response.ok && keyRecovery) {
        setRecovery(keyRecovery)
        setKeysMessage(data.message || 'The Bridge could not open the keys for your account.')
        setStep('keys')
        return
      }

      if (!response.ok || data.status === 'error') {
        setStep('password')
        setError(data.message || 'Login failed. Please try again.')
        if (data.redirect) {
          navigate(data.redirect)
        }
      } else if (data.status === 'totp_required') {
        // Server has accepted the password but won't issue a session
        // until TOTP verifies. Switch to the second-factor step.
        setStep('totp')
        setError(null)
      } else {
        // Set login state (broker from response if session was resumed, empty otherwise)
        setLogin(username, data.broker || '')
        showToast.success('Login successful', 'system')
        // Use redirect from response if provided, otherwise go to broker
        navigate(data.redirect || '/broker')
      }
    } catch (_err) {
      setError('Login failed. Please try again.')
    } finally {
      setIsLoading(false)
    }
  }

  const handleTotpSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    setIsLoading(true)
    setError(null)

    try {
      const csrfResponse = await fetch('/auth/csrf-token', { credentials: 'include' })
      if (!csrfResponse.ok) {
        setError('Failed to verify TOTP. Please refresh the page.')
        setIsLoading(false)
        return
      }
      const { csrf_token } = await csrfResponse.json()

      const response = await fetch('/auth/login/totp', {
        method: 'POST',
        credentials: 'include',
        headers: {
          'Content-Type': 'application/json',
          'X-CSRFToken': csrf_token,
        },
        body: JSON.stringify({ totp_code: totpCode }),
      })

      const data = await response.json()

      if (response.status === 401 && data.message?.toLowerCase().includes('expired')) {
        // Pending login window timed out — bounce back to password step.
        setError(data.message)
        setStep('password')
        setTotpCode('')
        return
      }

      if (!response.ok || data.status === 'error') {
        setError(data.message || 'Invalid TOTP code.')
        setTotpCode('')
        return
      }

      setLogin(username, data.broker || '')
      showToast.success('Login successful', 'system')
      navigate(data.redirect || '/broker')
    } catch (_err) {
      setError('Failed to verify TOTP. Please try again.')
    } finally {
      setIsLoading(false)
    }
  }

  const handleBackToPassword = () => {
    setStep('password')
    setTotpCode('')
    setError(null)
  }

  // Desktop: after a fresh start the trader signs in with the new password.
  const handleFreshStartDone = (message: string) => {
    showToast.success(message, 'system')
    setPassword('')
    setStep('password')
    setNotice(message)
  }

  // Show loading while checking setup
  if (isCheckingSetup) {
    return (
      <div className="min-h-screen flex items-center justify-center">
        <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
      </div>
    )
  }

  // Desktop: one clean sign-in card. After sign-in the server says where to
  // go: the dashboard when the saved broker session was resumed, otherwise
  // Select Broker.
  return (
    <div className="min-h-screen flex items-center justify-center bg-background py-8 px-4">
      <div className="w-full max-w-md">
        <Card className="w-full shadow-xl">
          <CardHeader className="text-center">
            <div className="flex justify-center mb-4">
              <img src="/logo.png" alt="The Bridge" className="h-16 w-16" />
            </div>
            <CardTitle className="text-2xl">
              <h1>The Bridge</h1>
            </CardTitle>
            <CardDescription>Sign in to your account</CardDescription>
          </CardHeader>
          <CardContent>
            {step === 'keys' ? (
              <div className="space-y-4">
                <Alert variant="destructive">
                  <KeyRound className="h-4 w-4" />
                  <AlertTitle>Your keys could not be opened</AlertTitle>
                  <AlertDescription>{keysMessage}</AlertDescription>
                </Alert>
                {recovery.includes('retry_keychain') && (
                  <Button
                    type="button"
                    className="w-full"
                    onClick={() => submitPassword()}
                    disabled={isLoading}
                  >
                    {isLoading ? (
                      <>
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                        Waiting for the keychain...
                      </>
                    ) : (
                      'Allow the keychain once and retry'
                    )}
                  </Button>
                )}
                {recovery.includes('start_fresh') && (
                  <Button
                    type="button"
                    variant="outline"
                    className="w-full"
                    onClick={() => setStep('fresh')}
                    disabled={isLoading}
                  >
                    Start fresh
                  </Button>
                )}
                <Button
                  type="button"
                  variant="ghost"
                  className="w-full"
                  onClick={handleBackToPassword}
                  disabled={isLoading}
                >
                  <ArrowLeft className="mr-1 h-4 w-4" />
                  Back
                </Button>
              </div>
            ) : step === 'fresh' ? (
              <div className="space-y-4">
                <StartFreshCard reason="keychain" onDone={handleFreshStartDone} />
                <Button
                  type="button"
                  variant="ghost"
                  className="w-full"
                  onClick={() => setStep('keys')}
                >
                  <ArrowLeft className="mr-1 h-4 w-4" />
                  Back
                </Button>
              </div>
            ) : step === 'password' ? (
              <form onSubmit={handleSubmit} className="space-y-4">
                <div className="space-y-2">
                  <Label htmlFor="username">Username</Label>
                  <Input
                    id="username"
                    type="text"
                    placeholder="Enter your username"
                    value={username}
                    onChange={(e) => setUsername(e.target.value)}
                    required
                    disabled={isLoading}
                    autoComplete="username"
                  />
                </div>

                <div className="space-y-2">
                  <Label htmlFor="password">Password</Label>
                  <div className="relative">
                    <Input
                      id="password"
                      type={showPassword ? 'text' : 'password'}
                      placeholder="Enter your password"
                      value={password}
                      onChange={(e) => setPassword(e.target.value)}
                      required
                      disabled={isLoading}
                      autoComplete="current-password"
                      className="pr-10"
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="absolute right-0 top-0 h-full px-3 hover:bg-transparent"
                      onClick={() => setShowPassword(!showPassword)}
                      aria-label={showPassword ? 'Hide password' : 'Show password'}
                    >
                      {showPassword ? (
                        <EyeOff className="h-4 w-4 text-muted-foreground" />
                      ) : (
                        <Eye className="h-4 w-4 text-muted-foreground" />
                      )}
                    </Button>
                  </div>
                  <div className="text-right">
                    <Link
                      to="/reset-password"
                      className="text-sm text-muted-foreground hover:text-primary"
                    >
                      Forgot password?
                    </Link>
                  </div>
                </div>

                {notice && (
                  <Alert>
                    <AlertDescription>{notice}</AlertDescription>
                  </Alert>
                )}

                {error && (
                  <Alert variant="destructive">
                    <AlertDescription>{error}</AlertDescription>
                  </Alert>
                )}

                <Button type="submit" className="w-full" disabled={isLoading}>
                  {isLoading ? (
                    <>
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                      Signing in...
                    </>
                  ) : (
                    <>
                      <LogIn className="mr-2 h-4 w-4" />
                      Sign in
                    </>
                  )}
                </Button>

                {needsSetup && (
                  <p className="text-center text-sm text-muted-foreground">
                    First time?{' '}
                    <Link
                      to="/setup"
                      className="font-medium text-primary underline underline-offset-4"
                    >
                      Set up your account
                    </Link>
                  </p>
                )}
              </form>
            ) : (
              <form onSubmit={handleTotpSubmit} className="space-y-4">
                <Alert>
                  <ShieldCheck className="h-4 w-4" />
                  <AlertTitle>Two-factor authentication</AlertTitle>
                  <AlertDescription>
                    Enter the 6-digit code from your authenticator app to complete sign-in.
                  </AlertDescription>
                </Alert>

                <div className="space-y-2">
                  <Label htmlFor="totp_code">Authentication code</Label>
                  <Input
                    id="totp_code"
                    inputMode="numeric"
                    autoComplete="one-time-code"
                    pattern="[0-9]{6}"
                    maxLength={6}
                    placeholder="123456"
                    value={totpCode}
                    onChange={(e) => setTotpCode(e.target.value.replace(/\D/g, '').slice(0, 6))}
                    disabled={isLoading}
                    autoFocus
                    required
                    className="font-mono text-center text-lg tracking-widest"
                  />
                </div>

                {error && (
                  <Alert variant="destructive">
                    <AlertDescription>{error}</AlertDescription>
                  </Alert>
                )}

                <div className="flex gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    onClick={handleBackToPassword}
                    disabled={isLoading}
                  >
                    <ArrowLeft className="mr-1 h-4 w-4" />
                    Back
                  </Button>
                  <Button
                    type="submit"
                    className="flex-1"
                    disabled={isLoading || totpCode.length !== 6}
                  >
                    {isLoading ? (
                      <>
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                        Verifying...
                      </>
                    ) : (
                      'Verify code'
                    )}
                  </Button>
                </div>
              </form>
            )}
          </CardContent>
        </Card>
        {/* Desktop: the keys are locked until sign-in (KEY-01). */}
        <p className="mt-4 text-center text-xs text-muted-foreground">
          The Bridge keeps your saved keys locked with your password. Until you sign in, the API,
          market data, webhooks, AI clients and your broker connection stay off, and they go off
          again when you log out.
        </p>
      </div>
    </div>
  )
}

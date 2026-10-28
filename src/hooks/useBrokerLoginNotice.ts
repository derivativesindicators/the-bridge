import { useEffect } from 'react'
import { useSearchParams } from 'react-router'
import { toast } from 'sonner'

/** "zerodha" -> "Zerodha" (the server sends the broker id). */
export function brokerLabel(id: string): string {
  return id ? id.charAt(0).toUpperCase() + id.slice(1) : 'Broker'
}

/**
 * The Bridge 1.0.13: a finished broker sign-in lands on
 * `/dashboard?broker_login=success&broker=<id>`. Say so once, then drop the
 * flag so a reload does not repeat it.
 */
export function useBrokerLoginNotice() {
  const [params, setParams] = useSearchParams()
  useEffect(() => {
    if (params.get('broker_login') !== 'success') return
    const broker = params.get('broker') ?? ''
    toast.success(`${brokerLabel(broker)} login successful`)
    setParams({}, { replace: true })
  }, [params, setParams])
}

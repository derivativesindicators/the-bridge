import { useState } from 'react'
import { type HeldOrder, markHeldOrderNotPlaced } from '@/api/health'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { showToast } from '@/utils/toast'
import { readTime } from './sampleFreshness'
import type { HealthRead } from './useHealthReads'

const heldKey = (order: HeldOrder) => `${order.owner}:${order.id}`

/** What stays true whatever the page could read. */
const STILL_HELD =
  "An order The Bridge holds stays held and is not sent again. Refresh to try again; until then, check the broker's order book."

/**
 * Desktop: live orders the broker did not confirm and does not show, held
 * for the trader's decision (P0-3). Marking one not placed, after a
 * confirmation, lets its position be managed again; `onResolved` then
 * refreshes the page.
 *
 * A list the page could not read is never shown as empty (PROD-PAR-01):
 * the orders last read stay listed, marked with when they were read, and a
 * list never read, or empty when last read, says the orders are not known.
 * Only a list read now and empty shows nothing.
 */
export function UnconfirmedOrders({
  read,
  onResolved,
}: {
  read: HealthRead<HeldOrder[]>
  onResolved: () => void
}) {
  const [confirmingHeld, setConfirmingHeld] = useState<string | null>(null)

  const handleNotPlaced = async (order: HeldOrder) => {
    try {
      await markHeldOrderNotPlaced(order.owner, order.id)
      showToast.success('Marked as not placed. The position is managed again.', 'monitoring')
      setConfirmingHeld(null)
      onResolved()
    } catch (_error) {
      showToast.error(
        'Could not mark the order as not placed. Refresh the page and try again.',
        'monitoring'
      )
    }
  }

  const orders = read.data
  const notRefreshed = read.failedAt !== null
  if (!orders || orders.length === 0) {
    return notRefreshed ? <UnconfirmedNotKnown read={read} /> : null
  }
  return (
    <Card className="border-destructive/50">
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-destructive">
          Unconfirmed Orders ({orders.length})
        </CardTitle>
        <CardDescription>
          The broker did not confirm these orders and does not show them. They are not sent again.
          Check the broker's order book and trade book; if an order is not there, mark it as not
          placed to resume.
        </CardDescription>
        {notRefreshed && (
          <p className="text-sm text-yellow-800 dark:text-yellow-400">
            Could not refresh this list at {readTime(read.failedAt)}; shown as read at{' '}
            {readTime(read.readAt)}. An order may have been found or settled since.
          </p>
        )}
      </CardHeader>
      <CardContent>
        <div className="space-y-3">
          {orders.map((order) => (
            <div
              key={heldKey(order)}
              className="rounded-lg border border-red-500/50 bg-red-500/10 p-3 flex items-start justify-between gap-4"
            >
              <div>
                <p className="font-medium">
                  {order.action} {order.quantity} {order.symbol} ({order.exchange})
                </p>
                <p className="text-sm text-neutral-600 dark:text-muted-foreground">
                  {order.message}
                </p>
                <p className="text-xs text-neutral-600 dark:text-muted-foreground">
                  Since {order.since}
                </p>
              </div>
              {confirmingHeld === heldKey(order) ? (
                <div className="flex gap-2">
                  <Button variant="destructive" size="sm" onClick={() => handleNotPlaced(order)}>
                    Confirm not placed
                  </Button>
                  <Button variant="outline" size="sm" onClick={() => setConfirmingHeld(null)}>
                    Cancel
                  </Button>
                </div>
              ) : (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setConfirmingHeld(heldKey(order))}
                >
                  Not placed
                </Button>
              )}
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}

/** The list could not be read now, and there is none to show as last read. */
function UnconfirmedNotKnown({ read }: { read: HealthRead<HeldOrder[]> }) {
  return (
    <Card className="border-yellow-500/50">
      <CardHeader>
        <CardTitle className="text-yellow-800 dark:text-yellow-400">
          Unconfirmed Orders: not known
        </CardTitle>
        <CardDescription>
          {read.data === null
            ? 'The Bridge could not read the orders waiting for your decision, so this page cannot show whether there are any.'
            : `There were none when last read at ${readTime(read.readAt)}, but the refresh at ${readTime(read.failedAt)} failed, so a newer one may not show.`}{' '}
          {STILL_HELD}
        </CardDescription>
      </CardHeader>
    </Card>
  )
}

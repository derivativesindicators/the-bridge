import { type ReactNode, useEffect } from 'react'
import type { UpdateStatus } from '@/api/updates'
import { useSocketContext } from '@/components/socket/SocketProvider'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { isDesktopShell, openExternal } from '@/lib/desktop'
import { type PendingWarning, useUpdateStore } from '@/stores/updateStore'

/** Socket.IO event the local server sends whenever the update state changes. */
export const UPDATE_STATUS_EVENT = 'update_status'

/** States in which the banner offers the update. */
const OFFERED = new Set(['available', 'downloading', 'ready', 'failed'])

function percentOf(status: UpdateStatus): number | null {
  const p = status.progress
  if (!p?.total) return null
  return Math.min(100, Math.floor((p.received * 100) / p.total))
}

/**
 * Desktop-only: the update notice. Reads the update state once, follows
 * the update_status push, and shows a small banner when a newer version is
 * available plus the update dialog (also opened by Check for Updates in the
 * profile menu). Nothing downloads until the trader chooses Update.
 * Mounted by the layouts, inside SocketProvider.
 */
export function UpdateNotice() {
  const { socket } = useSocketContext()

  useEffect(() => {
    void useUpdateStore.getState().load()
  }, [])

  useEffect(() => {
    if (!socket) return
    const onStatus = (status: UpdateStatus) => useUpdateStore.getState().setStatus(status)
    socket.on(UPDATE_STATUS_EVENT, onStatus)
    return () => {
      socket.off(UPDATE_STATUS_EVENT, onStatus)
    }
  }, [socket])

  return (
    <>
      <UpdateBanner />
      <UpdateDialog />
    </>
  )
}

function UpdateBanner() {
  const status = useUpdateStore((s) => s.status)
  const dialogOpen = useUpdateStore((s) => s.dialogOpen)
  const dismissedVersion = useUpdateStore((s) => s.dismissedVersion)
  const openDialog = useUpdateStore((s) => s.openDialog)
  const dismiss = useUpdateStore((s) => s.dismiss)
  const offer = status?.offer
  if (!status || !offer || dialogOpen || !OFFERED.has(status.state)) return null
  if (dismissedVersion === offer.version) return null
  const percent = percentOf(status)
  const headline =
    status.state === 'downloading'
      ? `Downloading The Bridge ${offer.version}${percent === null ? '' : `: ${percent}%`}`
      : status.state === 'ready'
        ? `The Bridge ${offer.version} is ready to install.`
        : `The Bridge ${offer.version} is available.`
  return (
    <section
      aria-label="Update available"
      className="fixed right-4 bottom-20 z-50 w-80 rounded-lg border bg-background p-4 shadow-lg md:bottom-4"
    >
      <p className="font-medium text-sm">{headline}</p>
      <p className="mt-1 text-muted-foreground text-xs">
        You have version {status.current_version}.
      </p>
      <div className="mt-3 flex justify-end gap-2">
        <Button size="sm" variant="ghost" onClick={dismiss}>
          Later
        </Button>
        <Button size="sm" onClick={openDialog}>
          See what is new
        </Button>
      </div>
    </section>
  )
}

function WarningView({ pending }: { pending: PendingWarning }) {
  const { warning } = pending
  return (
    <Alert variant={warning.level === 'live' ? 'destructive' : 'warning'}>
      <AlertTitle>{warning.title}</AlertTitle>
      <AlertDescription>
        <p>{warning.intro}</p>
        <ul className="mt-2 list-disc space-y-1 pl-5">
          {warning.items.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
        <p className="mt-2">{warning.advice}</p>
      </AlertDescription>
    </Alert>
  )
}

function ReleaseNotes({ notes }: { notes: string | null }) {
  if (!notes) {
    return <p className="text-muted-foreground text-sm">The release page lists what is new.</p>
  }
  return (
    <section
      aria-label="Release notes"
      className="max-h-64 overflow-y-auto whitespace-pre-wrap rounded-md border bg-muted/30 p-3 text-sm"
    >
      {notes}
    </section>
  )
}

function UpdateDialog() {
  const s = useUpdateStore()
  const status = s.status
  const offer = status?.offer ?? null
  const desktop = isDesktopShell()
  const releasePage = offer?.release_url ?? status?.releases_url ?? null

  const openReleasePage = () => {
    if (releasePage) void openExternal(releasePage)
  }

  let title = 'Check for updates'
  let description = ''
  let body: ReactNode = null
  let footer: ReactNode = (
    <Button variant="outline" onClick={s.closeDialog}>
      Close
    </Button>
  )

  if (s.pending) {
    const live = s.pending.warning.level === 'live'
    const pending = s.pending
    title = offer ? `Update to ${offer.version}?` : 'Update The Bridge?'
    description = 'Installing the update restarts The Bridge.'
    body = <WarningView pending={pending} />
    footer = (
      <>
        <Button variant="outline" onClick={() => useUpdateStore.setState({ pending: null })}>
          Not now
        </Button>
        <Button
          variant={live ? 'destructive' : 'default'}
          disabled={s.busy}
          onClick={() =>
            pending.step === 'install'
              ? s.startUpdate(pending.activity)
              : s.restart(pending.activity)
          }
        >
          {live ? 'Update and restart anyway' : 'Update and restart'}
        </Button>
      </>
    )
  } else if (!status || status.state === 'checking' || (s.busy && !offer)) {
    title = 'Checking for updates'
    description = 'The Bridge is asking for the newest version.'
  } else if (status.state === 'up_to_date' || status.state === 'not_checked') {
    title = 'The Bridge is up to date'
    description =
      status.state === 'up_to_date'
        ? `You are on the latest version, ${status.current_version}.`
        : `You are on version ${status.current_version}.`
    footer = (
      <>
        <Button variant="outline" onClick={s.closeDialog}>
          Close
        </Button>
        <Button onClick={s.checkNow} disabled={s.busy}>
          Check again
        </Button>
      </>
    )
  } else if (!offer) {
    // A failed check.
    title = 'Could not check for updates'
    description = `You are on version ${status.current_version}.`
    footer = (
      <>
        <Button variant="outline" onClick={s.closeDialog}>
          Close
        </Button>
        <Button onClick={s.checkNow} disabled={s.busy}>
          Try again
        </Button>
      </>
    )
  } else if (status.state === 'downloading') {
    const percent = percentOf(status)
    title = `Updating to ${offer.version}`
    description = 'Downloading the update and checking its signature.'
    body = (
      <div
        role="progressbar"
        aria-label="Download progress"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent ?? undefined}
        className="h-2 w-full overflow-hidden rounded-full bg-secondary"
      >
        <div className="h-full bg-primary transition-all" style={{ width: `${percent ?? 5}%` }} />
      </div>
    )
    footer = (
      <Button variant="outline" onClick={s.closeDialog}>
        Hide
      </Button>
    )
  } else if (status.state === 'ready') {
    title = `Version ${offer.version} is ready`
    description = desktop
      ? 'The update is downloaded and its signature checked. Restart The Bridge to install it.'
      : 'The update is downloaded. Open The Bridge on this computer to install it.'
    footer = (
      <>
        <Button variant="outline" onClick={s.later}>
          Later
        </Button>
        {desktop && (
          <Button onClick={() => s.restart(s.confirmed)} disabled={s.busy}>
            Restart now
          </Button>
        )}
      </>
    )
  } else if (status.state === 'restarting') {
    title = 'Restarting The Bridge'
    description = 'The Bridge is restarting to finish the update.'
    footer = null
  } else {
    // available, or a failed download with the offer kept.
    title = `The Bridge ${offer.version} is available`
    description = `You have version ${status.current_version}.`
    body = (
      <div className="space-y-3">
        <ReleaseNotes notes={offer.notes} />
        {status.can_install ? (
          <p className="text-muted-foreground text-sm">
            {desktop
              ? 'Update downloads the new version, checks its signature, installs it and restarts The Bridge. Nothing is downloaded until you choose Update.'
              : 'Open The Bridge on this computer to install it.'}
          </p>
        ) : (
          <p className="text-sm">{status.manual_reason}</p>
        )}
      </div>
    )
    footer = status.can_install ? (
      <>
        <Button variant="outline" onClick={s.later}>
          Later
        </Button>
        {desktop && (
          <Button onClick={() => s.startUpdate()} disabled={s.busy}>
            {status.state === 'failed' ? 'Try again' : 'Update'}
          </Button>
        )}
      </>
    ) : (
      <>
        <Button variant="outline" onClick={s.later}>
          Later
        </Button>
        <Button onClick={openReleasePage}>Open the release page</Button>
      </>
    )
  }

  const message = s.error ?? (status?.state === 'failed' ? status.message : null)

  return (
    <Dialog open={s.dialogOpen} onOpenChange={(open) => (open ? s.openDialog() : s.closeDialog())}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          {description && <DialogDescription>{description}</DialogDescription>}
        </DialogHeader>
        {message && !s.pending && (
          <Alert variant="destructive">
            <AlertDescription>{message}</AlertDescription>
          </Alert>
        )}
        {body}
        {footer && <DialogFooter>{footer}</DialogFooter>}
      </DialogContent>
    </Dialog>
  )
}

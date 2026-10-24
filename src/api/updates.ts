/**
 * Desktop-only: in-app updates. OpenAlgo web is updated with git pull; the
 * desktop app checks for a newer version, tells the trader and installs it
 * only when they choose Update.
 *
 * The local server decides everything (which version is newer, what a
 * restart would interrupt, what the trader confirmed). The restart itself
 * is the desktop window's own command (restartToUpdate in lib/desktop).
 */

import { webClient } from './client'

export type UpdateState =
  | 'not_checked'
  | 'checking'
  | 'up_to_date'
  | 'available'
  | 'downloading'
  | 'ready'
  | 'restarting'
  | 'failed'

/** How this copy was installed; only mac_app, windows_nsis and linux_app_image update themselves. */
export type UpdatePackage =
  | 'mac_app'
  | 'windows_nsis'
  | 'linux_app_image'
  | 'linux_deb'
  | 'linux_rpm'
  | 'development'
  | 'unknown'

export interface UpdateOffer {
  version: string
  notes: string | null
  published_at: string | null
  release_url: string
}

export interface UpdateStatus {
  current_version: string
  state: UpdateState
  /** Why the last step failed, written for the trader. */
  message: string | null
  progress: { received: number; total: number | null } | null
  offer: UpdateOffer | null
  package: UpdatePackage
  /** This copy can download and install the update itself. */
  can_install: boolean
  /** Why it cannot (a .deb install, a build from source). */
  manual_reason: string | null
  releases_url: string
  checked_at: string | null
}

/** What a restart would interrupt; sent back as the trader's confirmation. */
export interface UpdateActivity {
  live_strategies: number
  live_openscript_runs: number
  live_scalping_stops: number
  live_open_orders: number
  live_orders_unknown: boolean
  unsettled_orders: number
  checks_failed: boolean
  sandbox_strategies: number
  sandbox_openscript_runs: number
  sandbox_scalping_stops: number
  sandbox_open_orders: number
}

export interface UpdateWarning {
  /** live: Live trading would go unwatched; sandbox: only sandbox activity pauses. */
  level: 'live' | 'sandbox'
  title: string
  intro: string
  items: string[]
  advice: string
}

export type InstallAnswer =
  | { status: 'success'; data: UpdateStatus }
  | { status: 'confirm'; activity: UpdateActivity; warning: UpdateWarning }
  | { status: 'error'; message: string }

interface Envelope {
  status: string
  data?: UpdateStatus
  message?: string
}

function statusOf(body: Envelope): UpdateStatus {
  if (body.status !== 'success' || !body.data) {
    throw new Error(body.message || 'The Bridge could not read the update status. Try again.')
  }
  return body.data
}

export const updatesApi = {
  async status(): Promise<UpdateStatus> {
    const res = await webClient.get<Envelope>('/api/update/status')
    return statusOf(res.data)
  },

  async check(): Promise<UpdateStatus> {
    const res = await webClient.post<Envelope>('/api/update/check', {})
    return statusOf(res.data)
  },

  /** Start the update; `confirmed` is the activity the trader saw in the warning. */
  async install(confirmed?: UpdateActivity): Promise<InstallAnswer> {
    const res = await webClient.post<InstallAnswer>(
      '/api/update/install',
      confirmed ? { confirmed } : {},
      // A refusal (409) carries the trader's message; read it, not throw.
      { validateStatus: (s) => s < 500 }
    )
    const body = res.data
    if (body && (body.status === 'success' || body.status === 'confirm')) return body
    return {
      status: 'error',
      message:
        (body as { message?: string } | undefined)?.message ||
        'The update could not be started. Try again.',
    }
  },

  /** Forget a downloaded update (Later). */
  async cancel(): Promise<UpdateStatus> {
    const res = await webClient.post<Envelope>('/api/update/cancel', {})
    return statusOf(res.data)
  },
}

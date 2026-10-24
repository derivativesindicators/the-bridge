/**
 * Desktop-only: the in-app update flow as the window sees it. The state
 * itself comes from the local server (GET /api/update/status and the
 * update_status push); this store adds what belongs to the window: the
 * dialog, a banner dismissed for this version, the warning on screen and
 * what the trader confirmed.
 */

import { create } from 'zustand'
import {
  type UpdateActivity,
  type UpdateStatus,
  type UpdateWarning,
  updatesApi,
} from '@/api/updates'
import { isDesktopShell, restartToUpdate } from '@/lib/desktop'

export interface PendingWarning {
  warning: UpdateWarning
  activity: UpdateActivity
  /** install: before the download; restart: new activity appeared since. */
  step: 'install' | 'restart'
}

interface UpdateStore {
  status: UpdateStatus | null
  dialogOpen: boolean
  /** The version whose banner the trader closed with Later (until restart). */
  dismissedVersion: string | null
  pending: PendingWarning | null
  /** What the trader confirmed in the last warning. */
  confirmed: UpdateActivity | null
  /** The trader chose Update in this window: restart once it is ready. */
  restartWhenReady: boolean
  error: string | null
  busy: boolean
  setStatus: (status: UpdateStatus) => void
  load: () => Promise<void>
  checkNow: () => Promise<void>
  openDialog: () => void
  closeDialog: () => void
  dismiss: () => void
  startUpdate: (confirmed?: UpdateActivity) => Promise<void>
  restart: (confirmed?: UpdateActivity | null) => Promise<void>
  later: () => Promise<void>
}

const COULD_NOT_REACH = 'Could not reach The Bridge. Try again.'

function messageOf(e: unknown): string {
  return e instanceof Error && e.message ? e.message : COULD_NOT_REACH
}

export const useUpdateStore = create<UpdateStore>((set, get) => ({
  status: null,
  dialogOpen: false,
  dismissedVersion: null,
  pending: null,
  confirmed: null,
  restartWhenReady: false,
  error: null,
  busy: false,

  setStatus: (status) => {
    set({ status })
    const s = get()
    if (status.state === 'ready' && s.restartWhenReady && !s.busy) {
      set({ restartWhenReady: false })
      void s.restart(s.confirmed)
    }
  },

  load: async () => {
    try {
      get().setStatus(await updatesApi.status())
    } catch {
      // The banner simply does not show; the dialog reads it again.
    }
  },

  checkNow: async () => {
    set({ dialogOpen: true, error: null, pending: null, busy: true })
    try {
      get().setStatus(await updatesApi.check())
    } catch (e) {
      set({ error: messageOf(e) })
    } finally {
      set({ busy: false })
    }
  },

  openDialog: () => set({ dialogOpen: true, error: null }),

  closeDialog: () => set({ dialogOpen: false, pending: null, error: null }),

  dismiss: () => set({ dismissedVersion: get().status?.offer?.version ?? null }),

  startUpdate: async (confirmed) => {
    if (!isDesktopShell()) {
      set({ error: 'Open The Bridge on this computer to install the update.' })
      return
    }
    set({ busy: true, error: null, pending: null, dialogOpen: true })
    try {
      const answer = await updatesApi.install(confirmed)
      if (answer.status === 'confirm') {
        set({
          pending: { warning: answer.warning, activity: answer.activity, step: 'install' },
        })
      } else if (answer.status === 'error') {
        set({ error: answer.message })
      } else {
        set({ confirmed: confirmed ?? null, restartWhenReady: true })
        set({ busy: false })
        get().setStatus(answer.data)
      }
    } catch (e) {
      set({ error: messageOf(e) })
    } finally {
      set({ busy: false })
    }
  },

  restart: async (confirmed) => {
    set({ busy: true, error: null, pending: null, dialogOpen: true })
    try {
      const out = await restartToUpdate(confirmed ?? get().confirmed)
      if (out.status === 'confirm' && out.warning && out.activity) {
        set({ pending: { warning: out.warning, activity: out.activity, step: 'restart' } })
      } else if (out.status !== 'restarting') {
        set({ error: out.message || 'The Bridge could not restart to install the update.' })
      }
    } finally {
      set({ busy: false })
    }
  },

  later: async () => {
    const st = get().status
    set({ dialogOpen: false, pending: null, error: null, restartWhenReady: false })
    get().dismiss()
    if (st?.state === 'ready') {
      try {
        get().setStatus(await updatesApi.cancel())
      } catch {
        // It stays downloaded; Restart now still works.
      }
    }
  },
}))

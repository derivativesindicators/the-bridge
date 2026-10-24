/**
 * Desktop: whether the Health Monitor's latest sample still describes the
 * app. A missing or stale sample is UNKNOWN, never the PASS it last showed;
 * its readings stay on screen, in neutral colours. So is a sample the page
 * could not refresh (PROD-PAR-01): its figures stay, marked with when they
 * were read, and a WARN or FAIL it last read stays in view as that, never
 * turned into a grey UNKNOWN by a failed read.
 */

import type { CurrentMetrics, HealthLevel } from '@/api/health'
import { type HealthRead, REFRESH_INTERVAL_MS } from './useHealthReads'

/** A sample older than two sampling periods no longer describes the app. */
const SAMPLE_STALE_AFTER_S = 120

const IST = 'Asia/Kolkata'

const CLOCK = new Intl.DateTimeFormat('en-IN', {
  timeZone: IST,
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
  hour12: true,
})

const DATE = new Intl.DateTimeFormat('en-IN', { timeZone: IST, day: 'numeric', month: 'short' })

/** The IST calendar day, to tell today from another day. */
const DAY = new Intl.DateTimeFormat('en-CA', {
  timeZone: IST,
  year: 'numeric',
  month: '2-digit',
  day: '2-digit',
})

/**
 * A time the page read or failed to read something, or a gap began, in
 * IST; with the date when it is not today, so a gap open since yesterday
 * does not read as one from this afternoon.
 */
export function readTime(ms: number | null, now: number = Date.now()): string {
  if (ms === null) return '-'
  const at = new Date(ms)
  const date = DAY.format(at) === DAY.format(new Date(now)) ? '' : `${DATE.format(at)}, `
  return `${date}${CLOCK.format(at)} IST`
}

/** What happens next after a read failed, and what the trader can do. */
const RETRY = `The Bridge tries again every ${REFRESH_INTERVAL_MS / 1000} seconds while Auto is on, or choose Refresh.`

/** What the trader reads about a part of the page whose latest read failed. */
export function notRefreshedText(read: HealthRead<unknown>, what: string): string {
  return read.data === null
    ? `Could not read ${what}. ${RETRY}`
    : `Could not refresh ${what} at ${readTime(read.failedAt)}; shown as read at ${readTime(read.readAt)}. ${RETRY}`
}

/**
 * The sample's age now: as the server reported it when the page read it,
 * plus the time since, so a page that cannot refresh does not keep saying
 * "30 s ago". Else from its timestamp.
 */
function sampleAgeSeconds(
  metrics: CurrentMetrics,
  readAt: number | null,
  now: number
): number | null {
  if (typeof metrics.sample_age_s === 'number') {
    const sinceRead = readAt === null ? 0 : Math.max(0, Math.round((now - readAt) / 1000))
    return metrics.sample_age_s + sinceRead
  }
  const taken = Date.parse(metrics.timestamp)
  if (Number.isNaN(taken)) return null
  return Math.max(0, Math.round((now - taken) / 1000))
}

export function formatAge(seconds: number): string {
  if (seconds < 120) return `${seconds} s ago`
  if (seconds < 7200) return `${Math.floor(seconds / 60)} min ago`
  return `${Math.floor(seconds / 3600)} h ago`
}

export interface SampleFreshness {
  /**
   * The overall status to show: the sample's; `unknown` when stale; after
   * a failed refresh, a WARN or FAIL last read (see `verdictReadAt`).
   */
  overallStatus: HealthLevel
  /**
   * When `overallStatus` is the verdict last read, kept because a refresh
   * failed: the time it was read, to show beside it. Else null.
   */
  verdictReadAt: number | null
  /** Seconds since the sample was taken, when known. */
  sampleAge: number | null
  /** Why the status is unknown, for the trader; `null` when fresh. */
  staleReason: string | null
  /** A card's status to show: its own, or `unknown` when stale. */
  cardStatus: (status: HealthLevel | undefined) => HealthLevel
}

export function sampleFreshness(
  read: HealthRead<CurrentMetrics>,
  now: number = Date.now()
): SampleFreshness {
  const currentMetrics = read.data
  const sampleAge = currentMetrics ? sampleAgeSeconds(currentMetrics, read.readAt, now) : null
  const notRefreshed = read.failedAt !== null
  const stale =
    notRefreshed ||
    (currentMetrics !== null &&
      (currentMetrics.overall_status === 'unknown' ||
        sampleAge === null ||
        sampleAge > SAMPLE_STALE_AFTER_S))
  // Bad news is not hidden by a failed read, and a PASS is never carried
  // past one: only a WARN or FAIL stays, as last read.
  const lastVerdict = currentMetrics?.overall_status
  const keptVerdict = notRefreshed && (lastVerdict === 'warn' || lastVerdict === 'fail')
  const overallStatus = keptVerdict
    ? lastVerdict
    : stale
      ? 'unknown'
      : (currentMetrics?.overall_status ?? 'unknown')
  const staleReason = !stale
    ? null
    : notRefreshed
      ? notRefreshedText(read, 'the health figures')
      : (currentMetrics?.reason ??
        `No health sample${sampleAge === null ? '' : ` for ${formatAge(sampleAge).replace(' ago', '')}`}. Health sampling may have stopped; restart The Bridge if this continues.`)
  const cardStatus = (status: HealthLevel | undefined): HealthLevel =>
    stale ? 'unknown' : status || 'unknown'
  return {
    overallStatus,
    verdictReadAt: keptVerdict ? read.readAt : null,
    sampleAge,
    staleReason,
    cardStatus,
  }
}

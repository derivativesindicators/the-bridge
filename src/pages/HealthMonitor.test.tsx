import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { CurrentMetrics, EmptyHealthStats, HealthStats, HistoricalMetric } from '@/api/health'
import { render, screen, userEvent, waitFor, within } from '@/test/test-utils'
import HealthMonitor from './HealthMonitor'

const api = vi.hoisted(() => ({
  getCurrentMetrics: vi.fn(),
  getMetricsHistory: vi.fn(),
  getHealthStats: vi.fn(),
  getActiveAlerts: vi.fn(),
  getHeldOrders: vi.fn(),
  markHeldOrderNotPlaced: vi.fn(),
  acknowledgeAlert: vi.fn(),
  resolveAlert: vi.fn(),
}))

vi.mock('@/api/health', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/api/health')>()),
  ...api,
}))

// The charts draw on a canvas jsdom does not have.
vi.mock('lightweight-charts', () => {
  const stub: unknown = new Proxy(() => stub, {
    get: (_target, key) => (key === 'then' ? undefined : stub),
  })
  return { createChart: () => stub, LineSeries: {}, ColorType: { Solid: 'solid' } }
})

function metrics(overrides: Partial<CurrentMetrics> = {}): CurrentMetrics {
  return {
    timestamp: new Date().toISOString(),
    fd: { count: 120, limit: 10240, usage_percent: 1.2, status: 'pass' },
    memory: {
      rss_mb: 210,
      vms_mb: 900,
      percent: 2.5,
      available_mb: 4000,
      swap_mb: 0,
      status: 'pass',
    },
    database: { total: 3, connections: { openalgo: 2, logs: 1 }, status: 'pass' },
    websocket: { total: 0, connections: {}, total_symbols: 0, status: 'pass' },
    threads: { count: 40, stuck: 0, status: 'pass' },
    processes: [],
    overall_status: 'pass',
    ...overrides,
  }
}

/** A Live order held for the trader (P0-3). */
const HELD = {
  owner: 'strategy',
  id: '42',
  symbol: 'NIFTY07OCT2624500CE',
  exchange: 'NFO',
  action: 'BUY',
  quantity: 65,
  since: '2026-10-07 10:00:20',
  message: 'It may still be at the broker.',
}

function alert(id: number, alert_type: string, resolvable: boolean) {
  return {
    id,
    timestamp: '2026-10-20T10:00:00+05:30',
    alert_type,
    severity: 'warn' as const,
    metric_name: alert_type,
    metric_value: 1,
    threshold_value: 0,
    message: `${alert_type} message`,
    acknowledged: true,
    resolved: false,
    resolvable,
  }
}

async function refresh(times: number) {
  await userEvent.click(screen.getByRole('button', { name: 'Refresh' }))
  await waitFor(() => expect(api.getCurrentMetrics).toHaveBeenCalledTimes(times))
  await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh' })).toBeEnabled())
}

/** The card with this title. */
function card(title: string): HTMLElement {
  const found = screen.getByText(title).closest<HTMLElement>('[data-slot="card"]')
  if (!found) throw new Error(`no card titled ${title}`)
  return found
}

/** One history sample, as `/health/api/history` answers it. */
const SAMPLE: HistoricalMetric = {
  timestamp: '2026-10-20T10:00:00+05:30',
  fd_count: 4321,
  memory_rss_mb: 210.5,
  db_connections: 3,
  ws_connections: 1,
  threads: 40,
  overall_status: 'pass',
}

/** A statistics window with samples. */
const STATS: HealthStats = {
  total_samples: 2,
  time_period_hours: 24,
  fd: { current: 120, avg: 118.5, min: 117, max: 120, fail_count: 0, warn_count: 0 },
  memory: {
    current_mb: 210,
    avg_mb: 205.25,
    min_mb: 200.5,
    max_mb: 210,
    fail_count: 0,
    warn_count: 0,
  },
  database: { current: 3, avg: 2.5, min: 2, max: 3 },
  websocket: { current: 3, avg: 2.5, min: 2, max: 3 },
  threads: { current: 3, avg: 2.5, min: 2, max: 3 },
  status: { overall: { pass: 2, warn: 0, fail: 0 } },
}

/** What the server answers for a window with no samples (`stats_json` on no rows). */
const NO_SAMPLES: EmptyHealthStats = {
  total_samples: 0,
  time_period_hours: 24,
  fd: {},
  memory: {},
  database: {},
  websocket: {},
  threads: {},
  status: {},
}

describe('Health Monitor', () => {
  beforeEach(() => {
    api.getCurrentMetrics.mockResolvedValue(metrics())
    api.getMetricsHistory.mockResolvedValue([])
    api.getHealthStats.mockResolvedValue(NO_SAMPLES)
    api.getActiveAlerts.mockResolvedValue([])
    api.getHeldOrders.mockResolvedValue([])
    api.markHeldOrderNotPlaced.mockResolvedValue(undefined)
    api.acknowledgeAlert.mockResolvedValue(undefined)
    api.resolveAlert.mockResolvedValue(undefined)
  })

  afterEach(() => {
    vi.clearAllMocks()
  })

  // The page used to read stats.fd.avg.toFixed on an empty window and crash.
  it('renders an empty statistics window instead of crashing', async () => {
    render(<HealthMonitor />)

    expect(
      await screen.findByText(/No health samples in the last 24 hours yet/)
    ).toBeInTheDocument()
    expect(screen.queryByText('File Descriptor Stats')).not.toBeInTheDocument()
    expect(screen.getByText(/System Status: PASS/)).toBeInTheDocument()
  })

  // DIA-01: the banner said PASS over a sample that was minutes old, or
  // over no measurement at all.
  it('shows UNKNOWN with the reason and the age when the server says the sample is stale', async () => {
    api.getCurrentMetrics.mockResolvedValue(
      metrics({
        overall_status: 'unknown',
        sample_age_s: 180,
        reason: 'The latest health sample is 3 minutes old, so the state of The Bridge is unknown.',
      })
    )
    render(<HealthMonitor />)

    expect(await screen.findByText(/System Status: UNKNOWN/)).toBeInTheDocument()
    expect(screen.getByText(/The latest health sample is 3 minutes old/)).toBeInTheDocument()
    expect(screen.getByText(/3 min ago/)).toBeInTheDocument()
    expect(screen.queryByText(/System Status: PASS/)).not.toBeInTheDocument()
  })

  it('treats an old sample as UNKNOWN even when the server does not report its age', async () => {
    api.getCurrentMetrics.mockResolvedValue(
      metrics({ timestamp: new Date(Date.now() - 10 * 60_000).toISOString() })
    )
    render(<HealthMonitor />)

    expect(await screen.findByText(/System Status: UNKNOWN/)).toBeInTheDocument()
    expect(screen.getByText(/Health sampling may have stopped/)).toBeInTheDocument()
    expect(screen.getByText(/10 min ago/)).toBeInTheDocument()
  })

  it('keeps the sample status while the sample is fresh, with its age', async () => {
    api.getCurrentMetrics.mockResolvedValue(metrics({ overall_status: 'warn', sample_age_s: 30 }))
    render(<HealthMonitor />)

    expect(await screen.findByText(/System Status: WARN/)).toBeInTheDocument()
    expect(screen.getByText(/30 s ago/)).toBeInTheDocument()
    expect(screen.queryByText(/Health sampling may have stopped/)).not.toBeInTheDocument()
  })

  it('shows the statistics when the window has samples', async () => {
    const group = { current: 3, avg: 2.5, min: 2, max: 3 }
    api.getHealthStats.mockResolvedValue({
      total_samples: 2,
      time_period_hours: 24,
      fd: { current: 120, avg: 118.5, min: 117, max: 120, fail_count: 0, warn_count: 0 },
      memory: {
        current_mb: 210,
        avg_mb: 205.25,
        min_mb: 200.5,
        max_mb: 210,
        fail_count: 0,
        warn_count: 0,
      },
      database: group,
      websocket: group,
      threads: group,
      status: { overall: { pass: 2, warn: 0, fail: 0 } },
    })
    render(<HealthMonitor />)

    expect(await screen.findByText('File Descriptor Stats')).toBeInTheDocument()
    expect(screen.getByText('118.5')).toBeInTheDocument()
    expect(screen.getByText('2/0/0')).toBeInTheDocument()
    expect(screen.queryByText(/No health samples/)).not.toBeInTheDocument()
  })

  // P0-3: a live order held for the trader is listed with a two-step
  // "not placed" action that names the order's owner and id.
  it('lists held orders and marks one not placed after confirmation', async () => {
    api.getHeldOrders.mockResolvedValue([HELD])
    render(<HealthMonitor />)

    expect(await screen.findByText('Unconfirmed Orders (1)')).toBeInTheDocument()
    expect(screen.getByText('BUY 65 NIFTY07OCT2624500CE (NFO)')).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Not placed' }))
    expect(api.markHeldOrderNotPlaced).not.toHaveBeenCalled()
    await userEvent.click(screen.getByRole('button', { name: 'Confirm not placed' }))
    expect(api.markHeldOrderNotPlaced).toHaveBeenCalledWith('strategy', '42')
  })

  // The resolve endpoint existed with nothing calling it: an alert only the
  // trader settles (lost records) stayed on the page for good.
  it('resolves an alert only the trader settles, and offers no Resolve on one The Bridge clears', async () => {
    api.getActiveAlerts.mockResolvedValue([
      alert(7, 'records_not_saved', true),
      alert(8, 'fd_count_warn', false),
    ])
    render(<HealthMonitor />)

    expect(await screen.findByText('records_not_saved message')).toBeInTheDocument()
    expect(screen.getByText('fd_count_warn message')).toBeInTheDocument()
    const resolve = screen.getAllByRole('button', { name: 'Resolve' })
    expect(resolve).toHaveLength(1)
    api.getActiveAlerts.mockResolvedValue([alert(8, 'fd_count_warn', false)])
    await userEvent.click(resolve[0])
    expect(api.resolveAlert).toHaveBeenCalledWith(7)
    await waitFor(() =>
      expect(screen.queryByText('records_not_saved message')).not.toBeInTheDocument()
    )
  })

  it('draws the app checks the sample carries, with a warning where something was missed', async () => {
    api.getCurrentMetrics.mockResolvedValue(
      metrics({
        records: {
          status: 'warn',
          order_logs_dropped: 2,
          order_logs_failed: 0,
          monitoring_dropped: 0,
          monitoring_lost: 0,
          monitoring_retried_batches: 1,
        },
      })
    )
    render(<HealthMonitor />)

    expect(await screen.findByText('Records Saved')).toBeInTheDocument()
    expect(screen.getByText('2 not saved')).toBeInTheDocument()
    expect(screen.getByText(/Some records were not saved/)).toBeInTheDocument()
  })

  it('reaches Resolve from the keyboard and resolves with Enter', async () => {
    api.getActiveAlerts.mockResolvedValue([alert(7, 'records_not_saved', true)])
    render(<HealthMonitor />)

    expect(await screen.findByText('records_not_saved message')).toBeInTheDocument()
    const resolve = screen.getByRole('button', { name: 'Resolve' })
    for (let i = 0; i < 40 && document.activeElement !== resolve; i++) {
      await userEvent.tab()
    }
    expect(resolve).toHaveFocus()
    await userEvent.keyboard('{Enter}')
    expect(api.resolveAlert).toHaveBeenCalledWith(7)
  })

  // The audit's first probe: an overall FAIL with the order tracking reason
  // and the records and strategy price blocks in the same answer.
  it('shows the order tracking reason behind a FAIL beside the records and strategy prices', async () => {
    api.getCurrentMetrics.mockResolvedValue(
      metrics({
        overall_status: 'fail',
        order_facts: {
          status: 'fail',
          gaps: [{ mode: 'live', cause: 'event_loss', since: new Date().toISOString() }],
          running: true,
          passes: 0,
          book_reads: 0,
          book_read_failures: 11,
          book_reads_discarded: 0,
          gaps_opened: 1,
          gaps_closed: 0,
          unconfirmed_orders: 0,
        },
        records: {
          status: 'warn',
          order_logs_dropped: 17,
          order_logs_failed: 0,
          monitoring_dropped: 0,
          monitoring_lost: 0,
          monitoring_retried_batches: 0,
        },
        strategy_ticks: {
          status: 'warn',
          ticks_queued: 30,
          ticks_folded: 0,
          ticks_missed: 19,
          exits_waiting: 0,
          oldest_exit_wait_s: null,
          exits_slow: false,
        },
      })
    )
    render(<HealthMonitor />)

    expect(await screen.findByText(/System Status: FAIL/)).toBeInTheDocument()
    expect(
      screen.getByText(/an order update could not be delivered inside The Bridge/)
    ).toBeInTheDocument()
    expect(screen.getByText('17 not saved')).toBeInTheDocument()
    expect(screen.getByText(/Strategy risk management missed some prices/)).toBeInTheDocument()
  })
})

// PROD-PAR-01: a failed read must never empty what the page last showed,
// and a list never read is not a list with nothing in it.
describe('Health Monitor when a read fails', () => {
  beforeEach(() => {
    api.getCurrentMetrics.mockResolvedValue(metrics())
    api.getMetricsHistory.mockResolvedValue([])
    api.getHealthStats.mockResolvedValue(NO_SAMPLES)
    api.getActiveAlerts.mockResolvedValue([])
    api.getHeldOrders.mockResolvedValue([])
    api.markHeldOrderNotPlaced.mockResolvedValue(undefined)
  })

  afterEach(() => {
    vi.clearAllMocks()
  })

  // The audit's second probe: one held order, then one failed refresh of
  // the list while every other read succeeds.
  it('keeps an unconfirmed order listed, marked, when a refresh of the list fails', async () => {
    api.getHeldOrders.mockResolvedValueOnce([HELD]).mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    expect(await screen.findByText('Unconfirmed Orders (1)')).toBeInTheDocument()
    await refresh(2)

    expect(api.getHeldOrders).toHaveBeenCalledTimes(2)
    expect(screen.getByText('Unconfirmed Orders (1)')).toBeInTheDocument()
    expect(screen.getByText('BUY 65 NIFTY07OCT2624500CE (NFO)')).toBeInTheDocument()
    expect(
      screen.getByText(/Could not refresh this list at .* IST; shown as read at/)
    ).toBeInTheDocument()
    // A failed read decides nothing: the order is not released.
    expect(api.markHeldOrderNotPlaced).not.toHaveBeenCalled()

    // Read again and empty: the broker settled it, and nothing is shown.
    api.getHeldOrders.mockResolvedValue([])
    await refresh(3)
    expect(screen.queryByText(/Unconfirmed Orders/)).not.toBeInTheDocument()
    expect(api.markHeldOrderNotPlaced).not.toHaveBeenCalled()
  })

  it('says the unconfirmed orders are not known when their first read fails, until one succeeds', async () => {
    api.getHeldOrders.mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    expect(await screen.findByText('Unconfirmed Orders: not known')).toBeInTheDocument()
    expect(
      screen.getByText(/could not read the orders waiting for your decision/)
    ).toBeInTheDocument()
    expect(screen.queryByText(/Unconfirmed Orders \(/)).not.toBeInTheDocument()

    api.getHeldOrders.mockResolvedValue([])
    await refresh(2)
    expect(screen.queryByText(/Unconfirmed Orders/)).not.toBeInTheDocument()
  })

  it('says a list that was empty may no longer be, when its refresh fails', async () => {
    api.getHeldOrders.mockResolvedValueOnce([]).mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    await screen.findByText(/System Status: PASS/)
    expect(screen.queryByText(/Unconfirmed Orders/)).not.toBeInTheDocument()
    await refresh(2)
    expect(screen.getByText('Unconfirmed Orders: not known')).toBeInTheDocument()
    expect(screen.getByText(/There were none when last read at/)).toBeInTheDocument()
  })

  it('keeps the health figures and checks on screen, UNKNOWN and marked, when a refresh fails', async () => {
    api.getCurrentMetrics
      .mockResolvedValueOnce(
        metrics({
          sample_age_s: 5,
          records: {
            status: 'warn',
            order_logs_dropped: 2,
            order_logs_failed: 0,
            monitoring_dropped: 0,
            monitoring_lost: 0,
            monitoring_retried_batches: 0,
          },
        })
      )
      .mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    expect(await screen.findByText(/System Status: PASS/)).toBeInTheDocument()
    await refresh(2)

    expect(screen.getByText(/System Status: UNKNOWN/)).toBeInTheDocument()
    expect(
      screen.getByText(
        /^Could not refresh the health figures at .* IST; shown as read at .* IST\. The Bridge tries again every 10 seconds while Auto is on, or choose Refresh\.$/
      )
    ).toBeInTheDocument()
    expect(screen.getByText('Records Saved')).toBeInTheDocument()
    expect(screen.getByText('2 not saved')).toBeInTheDocument()
    expect(
      screen.getAllByText(/^As read at .* IST; the refresh at .* IST failed\.$/).length
    ).toBeGreaterThan(0)
  })

  // One failed refresh turned a red FAIL banner into a grey UNKNOWN.
  it('keeps a FAIL it last read, with when it was read, when a refresh fails', async () => {
    api.getCurrentMetrics
      .mockResolvedValueOnce(metrics({ sample_age_s: 5, overall_status: 'fail' }))
      .mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    expect(await screen.findByText('System Status: FAIL')).toBeInTheDocument()
    await refresh(2)

    expect(screen.getByText(/^System Status: FAIL when last read at .* IST$/)).toBeInTheDocument()
    expect(screen.queryByText(/System Status: UNKNOWN/)).not.toBeInTheDocument()
    expect(screen.getByText(/^Could not refresh the health figures at/)).toBeInTheDocument()
  })

  it('shows UNKNOWN and the checks as not known when health was never read', async () => {
    api.getCurrentMetrics.mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    expect(await screen.findByText(/System Status: UNKNOWN/)).toBeInTheDocument()
    expect(
      screen.getByText(
        'Could not read the health figures. The Bridge tries again every 10 seconds while Auto is on, or choose Refresh.'
      )
    ).toBeInTheDocument()
    expect(screen.getByText('Order Tracking')).toBeInTheDocument()
    expect(screen.getAllByText('Not known')).toHaveLength(4)
    expect(screen.queryByText('None stuck')).not.toBeInTheDocument()
  })

  it('keeps alerts listed, marked, when their refresh fails', async () => {
    api.getActiveAlerts
      .mockResolvedValueOnce([alert(7, 'records_not_saved', true)])
      .mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    expect(await screen.findByText('records_not_saved message')).toBeInTheDocument()
    await refresh(2)
    expect(screen.getByText('records_not_saved message')).toBeInTheDocument()
    expect(
      screen.getByText(/Could not refresh the alerts at .* IST; shown as read at/)
    ).toBeInTheDocument()
  })

  it('says the alerts could not be read instead of showing none', async () => {
    api.getActiveAlerts.mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    expect(
      await screen.findByText(
        'Could not read the alerts. The Bridge tries again every 10 seconds while Auto is on, or choose Refresh.'
      )
    ).toBeInTheDocument()
    expect(screen.getByText('Active Alerts')).toBeInTheDocument()
  })

  // The Recent Metrics table showed its header and no rows, and the charts
  // were blank, with the only mark in the status banner.
  it('says the recent samples and charts are not known when their read fails', async () => {
    api.getMetricsHistory.mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    await screen.findByText(/System Status: PASS/)
    const recent = within(card('Recent Metrics'))
    expect(recent.getByText(/^Could not read these samples\./)).toBeInTheDocument()
    expect(recent.queryByRole('table')).not.toBeInTheDocument()
    for (const chart of ['File Descriptors (6h)', 'Memory Usage (6h)']) {
      expect(within(card(chart)).getByText(/^Could not read this chart\./)).toBeInTheDocument()
    }
  })

  it('keeps the recent samples and charts, marked in their cards, when their refresh fails', async () => {
    api.getMetricsHistory
      .mockResolvedValueOnce([SAMPLE])
      .mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    expect(await screen.findByText('4321')).toBeInTheDocument()
    expect(within(card('Recent Metrics')).queryByText(/Could not/)).not.toBeInTheDocument()
    await refresh(2)

    const recent = within(card('Recent Metrics'))
    expect(recent.getByText('4321')).toBeInTheDocument()
    expect(
      recent.getByText(/^Could not refresh these samples at .* IST; shown as read at .* IST\./)
    ).toBeInTheDocument()
    for (const chart of ['File Descriptors (6h)', 'Memory Usage (6h)']) {
      expect(
        within(card(chart)).getByText(/^Could not refresh this chart at .* IST; shown as read at/)
      ).toBeInTheDocument()
    }
  })

  it('says the statistics are not known when their read fails, in a card of their own', async () => {
    api.getHealthStats.mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    await screen.findByText(/System Status: PASS/)
    expect(
      within(card('Statistics')).getByText(/^Could not read the statistics\./)
    ).toBeInTheDocument()
    expect(screen.queryByText('File Descriptor Stats')).not.toBeInTheDocument()
  })

  it('keeps the statistics, marked, when their refresh fails', async () => {
    api.getHealthStats.mockResolvedValueOnce(STATS).mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    expect(await screen.findByText('File Descriptor Stats')).toBeInTheDocument()
    expect(screen.queryByText('Statistics')).not.toBeInTheDocument()
    await refresh(2)

    expect(screen.getByText('File Descriptor Stats')).toBeInTheDocument()
    expect(screen.getByText('118.5')).toBeInTheDocument()
    expect(
      within(card('Statistics')).getByText(
        /^Could not refresh the statistics at .* IST; shown as read at .* IST\./
      )
    ).toBeInTheDocument()
  })

  // "No process data available" and "No thread details available" read as
  // empty lists when health had never been read.
  it('says the process and thread lists are not known, not empty, when health was never read', async () => {
    api.getCurrentMetrics.mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    await screen.findByText(/System Status: UNKNOWN/)
    for (const title of ['Top Memory Processes', 'Thread Details']) {
      expect(within(card(title)).getByText(/^Could not read this list\./)).toBeInTheDocument()
    }
    expect(screen.queryByText(/No process data available/)).not.toBeInTheDocument()
    expect(screen.queryByText(/No thread details available/)).not.toBeInTheDocument()
  })

  it('keeps the process list, marked, when a refresh of health fails', async () => {
    const proc = {
      pid: 4242,
      name: 'openalgo-desktop',
      rss_mb: 210,
      vms_mb: 900,
      memory_percent: 2.5,
    }
    api.getCurrentMetrics
      .mockResolvedValueOnce(metrics({ sample_age_s: 5, processes: [proc] }))
      .mockRejectedValue(new Error('unreachable'))
    render(<HealthMonitor />)

    expect(await screen.findByText('openalgo-desktop')).toBeInTheDocument()
    await refresh(2)

    const processes = within(card('Top Memory Processes'))
    expect(processes.getByText('openalgo-desktop')).toBeInTheDocument()
    expect(
      processes.getByText(/^Could not refresh this list at .* IST; shown as read at/)
    ).toBeInTheDocument()
    expect(
      within(card('Thread Details')).getByText(/^Could not refresh this list at/)
    ).toBeInTheDocument()
    expect(screen.getByText(/No thread details available/)).toBeInTheDocument()
  })
})

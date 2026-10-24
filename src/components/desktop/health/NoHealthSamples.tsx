import { type EmptyHealthStats, type HealthStats, hasHealthSamples } from '@/api/health'
import { Card, CardContent } from '@/components/ui/card'

/**
 * Desktop: an empty statistics window has no figures to show (the groups
 * come back empty), so the Health Monitor says why instead.
 */
export function NoHealthSamples({ stats }: { stats: HealthStats | EmptyHealthStats | null }) {
  if (!stats || hasHealthSamples(stats)) return null
  return (
    <Card>
      <CardContent className="p-4 text-sm text-muted-foreground">
        No health samples in the last {stats.time_period_hours} hours yet. The Bridge records one
        every minute while it runs; check back shortly.
      </CardContent>
    </Card>
  )
}

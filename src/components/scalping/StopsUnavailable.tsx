/**
 * Desktop addition (re-audit NEW-TR-07): the saved stop-losses could not be
 * read, so the stops shown are the last ones read and may be out of date.
 * The web shows an empty list instead, which reads as "nothing is
 * protected" while the server keeps enforcing the stops it holds.
 */
export const STOPS_UNAVAILABLE_TEXT =
  'Saved stop-losses could not be read; the stops shown may be out of date. The Bridge keeps enforcing the ones it holds.'

export function StopsUnavailable({ show }: { show: boolean }) {
  if (!show) return null
  return (
    <span role="alert" className="text-xs text-destructive">
      {STOPS_UNAVAILABLE_TEXT}
    </span>
  )
}

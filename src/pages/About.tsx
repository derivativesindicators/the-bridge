// The Bridge 1.0.11: About & licence. The "Appropriate Legal Notices" of
// AGPL-3.0 section 0 and section 5(d): copyright, no warranty, the licence
// and where to get the corresponding source. Linked from the footer on every
// screen. The licence text is bundled from the repository's License.md.

import { useEffect, useState } from 'react'
import { Link } from 'react-router'
import { ExternalAnchor } from '@/components/common/ExternalAnchor'
import { Footer } from '@/components/layout/Footer'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import licenseText from '../../License.md?raw'

export const SOURCE_URL = 'https://github.com/derivativesindicators/the-bridge'
export const UPSTREAM_URL = 'https://github.com/marketcalls/openalgo'
export const UPSTREAM_DESKTOP_URL = 'https://github.com/marketcalls/openalgo-desktop'
export const AGPL_URL = 'https://www.gnu.org/licenses/agpl-3.0.html'

const link = 'text-primary underline-offset-2 hover:underline'

export default function About() {
  const [version, setVersion] = useState('')
  useEffect(() => {
    fetch('/auth/app-info')
      .then((r) => r.json())
      .then((d) => d?.status === 'success' && d.version && setVersion(d.version))
      .catch(() => {})
  }, [])
  return (
    <div className="flex min-h-screen flex-col">
      <main className="container mx-auto max-w-3xl space-y-6 px-4 py-8">
        <div>
          <Link to="/" className="text-sm text-muted-foreground hover:underline">
            ← Back
          </Link>
          <h1 className="mt-2 text-2xl font-bold tracking-tight">About &amp; licence</h1>
        </div>

        <Card>
          <CardHeader>
            <CardTitle>The Bridge{version ? ` v${version}` : ''}</CardTitle>
          </CardHeader>
          <CardContent className="space-y-3 text-sm">
            <p>
              The Bridge is based on OpenAlgo (
              <ExternalAnchor href={UPSTREAM_URL} className={link}>
                {UPSTREAM_URL}
              </ExternalAnchor>
              ), licensed under the GNU Affero General Public License v3.0. It is derived from
              OpenAlgo Desktop (
              <ExternalAnchor href={UPSTREAM_DESKTOP_URL} className={link}>
                {UPSTREAM_DESKTOP_URL}
              </ExternalAnchor>
              ) and has been modified by Derivatives Indicators.
            </p>
            <div>
              <p className="font-medium">Copyright</p>
              <p>Original work Copyright 2024 OpenAlgo.</p>
              <p>Modifications Copyright 2026 Derivatives Indicators.</p>
            </div>
            <p>
              <span className="font-medium">No warranty.</span> This program is provided “as is”,
              without warranty of any kind, either expressed or implied, including, but not limited
              to, the implied warranties of merchantability and fitness for a particular purpose. In
              no event will any copyright holder be liable for damages arising out of its use (see
              sections 15 and 16 of the licence).
            </p>
            <p>
              You may redistribute and/or modify this program under the terms of the GNU Affero
              General Public License v3.0.
            </p>
            <div>
              <p className="font-medium">Source code</p>
              <ul className="list-disc pl-5">
                <li>
                  Corresponding source of this version:{' '}
                  <ExternalAnchor href={SOURCE_URL} className={link}>
                    {SOURCE_URL}
                  </ExternalAnchor>
                </li>
                <li>
                  Upstream OpenAlgo:{' '}
                  <ExternalAnchor href={UPSTREAM_URL} className={link}>
                    {UPSTREAM_URL}
                  </ExternalAnchor>
                </li>
                <li>
                  Upstream OpenAlgo Desktop:{' '}
                  <ExternalAnchor href={UPSTREAM_DESKTOP_URL} className={link}>
                    {UPSTREAM_DESKTOP_URL}
                  </ExternalAnchor>
                </li>
              </ul>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>GNU Affero General Public License v3.0</CardTitle>
          </CardHeader>
          <CardContent className="space-y-2 text-sm">
            <p>
              Also online at{' '}
              <ExternalAnchor href={AGPL_URL} className={link}>
                {AGPL_URL}
              </ExternalAnchor>
              .
            </p>
            <pre
              data-testid="license-text"
              className="max-h-96 overflow-auto whitespace-pre-wrap rounded border bg-muted/30 p-3 font-mono text-xs"
            >
              {licenseText}
            </pre>
          </CardContent>
        </Card>
      </main>
      <Footer />
    </div>
  )
}

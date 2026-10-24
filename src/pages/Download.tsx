import {
  Apple,
  ArrowLeft,
  Download as DownloadIcon,
  ExternalLink,
  Github,
  Monitor,
} from 'lucide-react'
import { Link } from 'react-router'
import { ExternalAnchor } from '@/components/common/ExternalAnchor'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { layoutRedirect } from '@/lib/desktop'
import { useAuthStore } from '@/stores/authStore'

// REBRAND: download page for The Bridge (installers are published on GitHub Releases).
const RELEASES = 'https://github.com/derivativesindicators/the-bridge/releases'
const LATEST = `${RELEASES}/latest`

const platforms = [
  {
    id: 'windows',
    title: 'Windows',
    icon: Monitor,
    file: 'Windows installer (.exe, 64-bit)',
    note: 'Windows 10 or 11.',
  },
  {
    id: 'macos',
    title: 'macOS',
    icon: Apple,
    file: 'macOS disk image (.dmg, Apple Silicon and Intel)',
    note: 'Not yet signed by Apple: right-click the app and choose Open the first time.',
  },
]

export default function Download() {
  const { isAuthenticated, user } = useAuthStore()
  // Signed in to the app (with or without a broker yet): back into the app.
  // /dashboard sends a session without a broker on to /broker itself.
  const inApp =
    layoutRedirect(user, isAuthenticated, '/dashboard') === null || Boolean(user?.username)
  return (
    <div className="mx-auto max-w-4xl py-4 space-y-8">
      <div>
        <Button variant="ghost" size="sm" asChild>
          <Link to={inApp ? '/dashboard' : '/'}>
            <ArrowLeft className="mr-2 h-4 w-4" />
            {inApp ? 'Back to Dashboard' : 'Back to Home'}
          </Link>
        </Button>
      </div>
      <div className="text-center space-y-2">
        <h1 className="text-3xl font-bold">Download The Bridge</h1>
        <p className="text-muted-foreground">
          Get the latest version. Installed copies check for updates and ask before installing.
        </p>
      </div>
      <div className="grid gap-6 md:grid-cols-2">
        {platforms.map((p) => (
          <Card key={p.id}>
            <CardHeader>
              <CardTitle className="flex items-center gap-2">
                <p.icon className="h-5 w-5" />
                {p.title}
              </CardTitle>
              <CardDescription>{p.file}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <Button asChild className="w-full">
                <ExternalAnchor href={LATEST}>
                  <DownloadIcon className="mr-2 h-4 w-4" />
                  Download The Bridge for {p.title}
                </ExternalAnchor>
              </Button>
              <p className="text-xs text-muted-foreground">{p.note}</p>
            </CardContent>
          </Card>
        ))}
      </div>
      <div className="flex flex-wrap justify-center gap-3">
        <Button variant="outline" asChild>
          <ExternalAnchor href={RELEASES}>
            <ExternalLink className="mr-2 h-4 w-4" />
            All releases
          </ExternalAnchor>
        </Button>
        <Button variant="outline" asChild>
          <ExternalAnchor href="https://github.com/derivativesindicators/the-bridge">
            <Github className="mr-2 h-4 w-4" />
            Source code
          </ExternalAnchor>
        </Button>
      </div>
      <p className="text-center text-xs text-muted-foreground">
        The Bridge is based on OpenAlgo Desktop and licensed under the GNU AGPL-3.0.
      </p>
    </div>
  )
}

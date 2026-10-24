import { Apple, Download as DownloadIcon, ExternalLink, Github, Monitor } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'

// REBRAND: download page for The Bridge (installers are published on GitHub Releases).
const RELEASES = 'https://github.com/derivativesindicators/the-bridge/releases'
const LATEST = `${RELEASES}/latest`

const platforms = [
  { id: 'windows', title: 'Windows', icon: Monitor, file: 'Windows installer (.exe, 64-bit)', note: 'Windows 10 or 11.' },
  { id: 'macos', title: 'macOS', icon: Apple, file: 'macOS disk image (.dmg, Apple Silicon and Intel)', note: 'Not yet signed by Apple: right-click the app and choose Open the first time.' },
]

export default function Download() {
  return (
    <div className="container mx-auto max-w-4xl px-4 py-10 space-y-8">
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
                <a href={LATEST} target="_blank" rel="noopener noreferrer">
                  <DownloadIcon className="mr-2 h-4 w-4" />
                  Download The Bridge for {p.title}
                </a>
              </Button>
              <p className="text-xs text-muted-foreground">{p.note}</p>
            </CardContent>
          </Card>
        ))}
      </div>
      <div className="flex flex-wrap justify-center gap-3">
        <Button variant="outline" asChild>
          <a href={RELEASES} target="_blank" rel="noopener noreferrer">
            <ExternalLink className="mr-2 h-4 w-4" />
            All releases
          </a>
        </Button>
        <Button variant="outline" asChild>
          <a href="https://github.com/derivativesindicators/the-bridge" target="_blank" rel="noopener noreferrer">
            <Github className="mr-2 h-4 w-4" />
            Source code
          </a>
        </Button>
      </div>
      <p className="text-center text-xs text-muted-foreground">
        The Bridge is based on OpenAlgo Desktop and licensed under the GNU AGPL-3.0.
      </p>
    </div>
  )
}

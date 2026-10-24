import { Github, Monitor } from 'lucide-react'
import { useEffect, useState } from 'react'
import { ExternalAnchor } from '@/components/common/ExternalAnchor'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { useSessionStore } from '@/stores/sessionStore'

interface FooterProps {
  className?: string
}

export function Footer({ className }: FooterProps) {
  const [version, setVersion] = useState<string>('')
  const activeSessionCount = useSessionStore((s) => s.activeSessionCount)

  useEffect(() => {
    const fetchVersion = async () => {
      try {
        const response = await fetch('/auth/app-info')
        const data = await response.json()
        if (data.status === 'success') {
          setVersion(data.version)
        }
      } catch (_error) {}
    }

    fetchVersion()
  }, [])

  return (
    <footer className={cn('mt-auto border-t bg-muted/30', className)}>
      <div className="container mx-auto px-4 py-6">
        <div className="flex flex-col md:flex-row items-center justify-center gap-2 md:gap-4 text-sm text-muted-foreground">
          <div className="flex items-center gap-2">
            <span>Copyright 2026 Derivatives Indicators</span>
            <span className="hidden md:inline">|</span>
            <ExternalAnchor
              href="https://derivativesindicators.com"
              className="text-primary hover:underline font-medium"
            >
              derivativesindicators.com
            </ExternalAnchor>
          </div>
          <span className="text-center">Based on OpenAlgo (AGPL-3.0)</span>
          <ExternalAnchor
            href="https://github.com/marketcalls/openalgo-desktop"
            className="text-primary hover:underline"
          >
            Upstream
          </ExternalAnchor>
          <ExternalAnchor
            data-rebrand="source"
            href="https://github.com/derivativesindicators/the-bridge"
            className="text-primary hover:underline"
          >
            Source code
          </ExternalAnchor>
          {version && (
            <>
              <span className="hidden md:inline">|</span>
              <Badge variant="secondary" className="gap-1">
                <span className="opacity-75">v</span>
                <span>{version}</span>
              </Badge>
            </>
          )}
          {activeSessionCount > 0 && (
            <>
              <span className="hidden md:inline">|</span>
              <Badge variant="outline" className="gap-1">
                <Monitor className="h-3 w-3" />
                <span>
                  {activeSessionCount} {activeSessionCount === 1 ? 'session' : 'sessions'}
                </span>
              </Badge>
            </>
          )}
        </div>

        {/* Social Links */}
        <div className="flex justify-center gap-2 mt-4">
          <Button variant="ghost" size="icon" asChild className="h-8 w-8">
            <ExternalAnchor
              href="https://github.com/derivativesindicators/the-bridge"
              aria-label="GitHub"
            >
              <Github className="h-4 w-4" />
            </ExternalAnchor>
          </Button>
          <Button
            data-rebrand="social-x"
            variant="ghost"
            size="icon"
            asChild
            className="h-8 w-8"
            title="Follow us on X"
          >
            <ExternalAnchor href="https://x.com/i_amArko" aria-label="X (Twitter)">
              <svg className="h-4 w-4" fill="currentColor" viewBox="0 0 24 24">
                <path d="M18.901 1.153h3.68l-8.04 9.19L24 22.846h-7.406l-5.8-7.584-6.638 7.584H.474l8.6-9.83L0 1.154h7.594l5.243 6.932ZM17.61 20.644h2.039L6.486 3.24H4.298Z" />
              </svg>
            </ExternalAnchor>
          </Button>
          <Button
            data-rebrand="social-youtube"
            variant="ghost"
            size="icon"
            asChild
            className="h-8 w-8"
            title="Subscribe on YouTube"
          >
            <ExternalAnchor
              href="https://www.youtube.com/@DerivativesIndicators"
              aria-label="YouTube"
            >
              <svg className="h-4 w-4" fill="currentColor" viewBox="0 0 24 24">
                <path d="M23.498 6.186a3.016 3.016 0 0 0-2.122-2.136C19.505 3.545 12 3.545 12 3.545s-7.505 0-9.377.505A3.017 3.017 0 0 0 .502 6.186C0 8.07 0 12 0 12s0 3.93.502 5.814a3.016 3.016 0 0 0 2.122 2.136c1.871.505 9.376.505 9.376.505s7.505 0 9.377-.505a3.015 3.015 0 0 0 2.122-2.136C24 15.93 24 12 24 12s0-3.93-.502-5.814zM9.545 15.568V8.432L15.818 12l-6.273 3.568z" />
              </svg>
            </ExternalAnchor>
          </Button>
        </div>
      </div>
    </footer>
  )
}

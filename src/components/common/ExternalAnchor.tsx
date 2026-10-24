import type { AnchorHTMLAttributes, MouseEvent } from 'react'
import { isDesktopShell, openExternal } from '@/lib/desktop'

/**
 * The Bridge: an http(s) link that opens in the system browser from the
 * desktop window. A plain target="_blank" anchor has nowhere to go inside the
 * Tauri window, so the click is handled here explicitly instead of relying on
 * the global window click handler (which never sees a click whose
 * propagation was stopped).
 */
export function ExternalAnchor({
  href,
  onClick,
  children,
  ...rest
}: AnchorHTMLAttributes<HTMLAnchorElement> & { href: string }) {
  const handle = (e: MouseEvent<HTMLAnchorElement>) => {
    onClick?.(e)
    if (e.defaultPrevented || !isDesktopShell()) return
    e.preventDefault()
    e.stopPropagation()
    void openExternal(href)
  }
  return (
    <a href={href} target="_blank" rel="noopener noreferrer" onClick={handle} {...rest}>
      {children}
    </a>
  )
}

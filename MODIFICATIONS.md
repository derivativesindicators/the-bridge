# Modifications

This program is a modified version of OpenAlgo Desktop (https://github.com/marketcalls/openalgo-desktop, Copyright 2024 OpenAlgo), licensed under the GNU AGPL-3.0 (see License.md). Modified versions carry this notice per AGPL-3.0 section 5(a).

## 2026-10-24 - Derivatives Indicators
- Renamed product to "The Bridge" (package `the-bridge`, app id `com.derivativesindicators.thebridge`); window titles and user-visible text.
- Replaced icons/logo with The Bridge logo (Navbar, favicon, installer icons). Footer: own website, 'Based on OpenAlgo (AGPL-3.0)', source-code link.
- Disabled upstream auto-updater endpoint/pubkey (placeholder for our own).
- Added one-time copy of the legacy `com.openalgo.desktop` data folder into the new app-id folder.
- Corresponding Source: https://github.com/derivativesindicators/the-bridge

## 2026-10-24 - Derivatives Indicators
- Renamed product to "The Bridge" (package `the-bridge`, app id `com.derivativesindicators.thebridge`); window titles and user-visible text.
- Replaced icons/logo. Footer: own website, 'Based on OpenAlgo (AGPL-3.0)', source-code link.
- Disabled upstream auto-updater endpoint/pubkey (placeholder for our own).
- Added one-time copy of the legacy `com.openalgo.desktop` data folder into the new app-id folder.
- Corresponding Source: https://github.com/derivativesindicators/the-bridge

## 2026-10-24 - Derivatives Indicators
- Renamed product to "The Bridge" (package `the-bridge`, app id `com.derivativesindicators.thebridge`); window titles and user-visible text.
- Replaced icons/logo. Footer: own website, 'Based on OpenAlgo (AGPL-3.0)', source-code link.
- Auto-updater points to our own GitHub releases, signed with our own key.
- Removed upstream community (Discord/X/YouTube) links from the footer.
- Added one-time copy of the legacy `com.openalgo.desktop` data folder into the new app-id folder.
- Corresponding Source: https://github.com/derivativesindicators/the-bridge

## 2026-10-24 - Derivatives Indicators (v1.0.3)
- Charts: the engine's default "Chart by OpenAlgo" mark (link to openalgo.in) now reads "Chart by The Bridge" with The Bridge logo, via a Vite build plugin (`vite-plugins/chartBranding.ts`); the "Based on OpenAlgo" credit stays in the footer.
- Market data: `MarketDataManager` now reconnects after a *clean* server close (4401 auth timeout / authentication expired, 1001) and after a failed config fetch; previously such a close left pages (Scalping Terminal) "Disconnected" for good while REST quotes kept prices moving.
- Logo/icons: logo composited on a black rounded square so its white parts are visible on light backgrounds; all Tauri icons, favicon and in-app logo regenerated (`rebrand.py --icon-bg`).
- External links (Download page, footer) open in the system browser through the app's own `open_external` command (permission added to the app window) with the shell plugin as fallback.
- Download page renders inside the app layout (header nav) when signed in, with a Back to Dashboard/Home button.
## 2026-10-25 - Derivatives Indicators (v1.0.4)
- Own default ports: HTTP 5050 and market data WebSocket 8770 (OpenAlgo Desktop uses 5000/8765), so both apps run side by side. Migration `096_the_bridge_own_ports` moves settings still on 5000/8765 (including ones copied from OpenAlgo Desktop's data folder); custom ports are kept.
- A taken port now names the likely cause ("most likely OpenAlgo Desktop or another copy of The Bridge"); the Scalping Terminal banner shows that message instead of only "Live feed lost". `/api/websocket/config` reports the port the feed actually listens on.
- External links: the shell plugin's open scope `^https?://` (anchored by the plugin as `^^https?://$`) refused every real URL, so Download page / footer links did nothing (logged: "Scoped command argument at position 0 was found, but failed regex validation ^^https?://$"). Now `^https?://.+`, with a test.
- Download page: "Back to Dashboard" whenever signed in to the app.

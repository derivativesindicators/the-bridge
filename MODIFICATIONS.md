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

## 2026-10-25 - Derivatives Indicators (v1.0.5)
- New: guided "Set up a permanent webhook URL" on the TradingView page (ngrok free dev domain, or a Cloudflare named tunnel on your own domain). The Bridge downloads the official agent on first use (not bundled, per ngrok's terms), runs and supervises it, stores the token encrypted (never logged), sets the Host Server URL to the public address, restricts tunnel callers to webhook routes, and offers "Test webhook". Code: `src-tauri/src/tunnel.rs`, `/api/tunnel*`, `src/components/tunnel/`. Docs: `docs/userguide/33-permanent-webhook-url`.

## 2026-10-25 - Derivatives Indicators (v1.0.6)
- TradingView page: symbol search now covers NSE, NSE_INDEX, NFO, BSE, BSE_INDEX, BFO, MCX (broker-supported) with exchange badges, ranks index/futures ahead of option walls, sets Exchange from the pick (Exchange now sits above Symbol), and warns that an index is not tradeable via placesmartorder. Upstream searched only the dropdown exchange (inherited). Code: `src/lib/symbolSearch.ts`, `src/pages/TradingView.tsx`.
- Strategies: signal-driven (TradingView) strategies on the index tabs (Weekly & Monthly, Monthly Only). Legs without a symbol trade the strategy's underlying - future or ATM-relative option - resolved when the alert arrives, each leg firing on long or short signals; entries flip the opposite side; exits close the opened contract; per-leg and overall risk apply. Upstream listed this as not built. Code: `strategy/validate/legs.rs`, `strategy/signals.rs`, `strategy/signals/entries.rs`, wizard and detail pages; tests `tests/it/strategy/signal_index_legs.rs`.

## 2026-10-25 - Derivatives Indicators (v1.0.7)
- Strategies: premium-based strike for index option legs in signal-driven strategies - "Premium ≥ X" (cheapest at or above X, the backtest engine's rule), "Premium ≤ X" (dearest at or below X), "Closest to X" - resolved when the alert arrives from one multi-quote of the expiry's CE/PE strikes around ATM (±10, widened to ±25). No quote or no qualifying strike places nothing for that leg, refuses the alert with the reason and emits a `leg_entry_rejected` warning. New `OrderGateway::ltps` (broker multiquotes). Code: `strategy/resolver.rs`, `strategy/validate/legs.rs`, `strategy/signals/entries.rs`, `strategy/dispatch.rs`, wizard leg editor and detail page; tests in `resolver.rs`, `tests/it/strategy/signal_index_legs.rs`, `strategy_module.test.ts`. Docs: user guide 16, `docs/api/strategy-services`.
- Strategies: signal strategy webhooks accept plain-text alert bodies (e.g. Pine `alert("BUY")`, "CLOSE LONG: Normal Exit") mapped by a per-strategy, editable "Webhook messages" list (defaults BUY/SELL/CLOSE LONG*/CLOSE SHORT*), case-insensitive and trimmed; JSON still accepted; the URL token remains the only credential. Migration `098_strategy_webhook_messages`. Code: `strategy/webhook/messages.rs`, `strategy/webhook/pipeline.rs`, store, validate, wizard; tests in `messages.rs`, `tests/it/strategy/signal_index_legs.rs`, `strategy_module.test.ts`.

## 2026-10-25 - Derivatives Indicators (v1.0.8)
- Strategies: optional signal-strategy behaviours, each off by default, under "Advanced signal options" in the wizard. (1) Overnight carry: Positional and NRML only; the daily loss limit uses a per-session P&L baseline (`RunState.day_base`, checkpointed and recovered). (2) Expiry-day roll: closes expiring legs at the roll time (before 15:30) and reopens them in the next expiry, with the strike chosen again, at the next session or immediately; pending reopens are persisted. Also entry deferral (minimum days to expiry, defer on expiry day). (3) CAS exit: a Supertrend on the traded option's premium, warmed from broker history (new `OrderGateway::history`) and built from live prices, exits on a flip. Session times are Auto (15:15–15:40; from 2026-12-07 15:30–15:45, and 15:30 on the expiring contract's expiry day) or Custom. Migration `099_strategy_signal_options`. Code: `strategy/signal_options.rs`, `strategy/signal_timers.rs`, `strategy/signals.rs`, `strategy/engine/ticks.rs`, `src/pages/strategy/SignalOptionsCard.tsx`. Tests: `tests/it/strategy/signal_options_it.rs`, `signal_options.rs` unit tests, `SignalOptionsCard.test.tsx`. Docs: user guide 16.

## 2026-10-26 - Derivatives Indicators (v1.0.11)
- Footer slimmed to the copyright line, website, an "About & licence" link and the version/session pills. The "Based on OpenAlgo (AGPL-3.0)" credit, upstream and source-code links moved to a new in-app About & licence page (`/about`, `src/pages/About.tsx`) linked from the footer, which carries the Appropriate Legal Notices (AGPL-3.0 sections 0 and 5(d)): copyright of OpenAlgo and Derivatives Indicators, no-warranty statement, the bundled licence text (License.md) and the corresponding-source and upstream links.
- Strategy Positions: side colours by market direction for options (CE long / PE short green, CE short / PE long red), also on Orderbook, Tradebook, Positions and the strategy Detail page; per-strategy "Close positions" and a "Close all open positions" button (both books) that square off each strategy's own legs by fixed quantity and leave the strategies running; strategies sharing a contract keep their own quantities on the desk and Detail page instead of the netted broker row.
- Strategy legs closed outside the strategy no longer stay open (`src-tauri/src/strategy/external.rs`): a leg on an expired contract is closed at the sandbox settlement price (Live: closed as expired with a warning) without sending an order, on startup, in the periodic check and before any exit; event `expiry_settled`. A contract that is flat in the book while strategies hold it (Positions page Close All, MIS auto square-off) closes the legs at the fill price with reason `external_close` or `auto_squareoff`. The Positions page Close All warns when running strategies hold positions in that book (Cancel / Close anyway) and reconciles afterwards (`/strategy/api/desk/open_legs`, `/strategy/api/desk/reconcile`).

## 2026-10-27 - Derivatives Indicators (v1.0.12)
- Strategies: a run whose stop was requested now finishes once every leg is flat. A signal-entry claim with no order row no longer keeps a stopping run managed, an open leg with a completed entry, qty 0 and nothing in flight counts as flat, and the stop request is announced once instead of every 5 s. Code: `strategy/state.rs`, `strategy/engine/stop.rs`; tests `tests/it/strategy/v1012_regressions.rs`.
- Sandbox: symbol lookups fall back to the stored master contract (`symtoken`) when the in-memory master was dropped at the 03:00 IST session boundary, so exits (e.g. a carried position on its expiry day) work before the morning broker sign-in. Code: `services/sandbox_feed.rs`, `db/sqlite/symbol.rs::find_symbol`.
- Strategy Positions / strategy detail: entry and exit fill times in IST beside the prices (date shown for an overnight carry). Code: `strategy/desk.rs`, `src/lib/istStamp.ts`, `src/pages/strategy/Desk.tsx`, `Detail.tsx`.

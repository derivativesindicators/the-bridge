//! The `research` toolset: indicators, snapshots, signals and screens on
//! history (`mcpserver.py` "RESEARCH TOOLS").

use crate::mcp::schema::{Dflt as D, Param, Ty::*};
use crate::mcp::{Risk, Scope, ToolDef};

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "calculate_indicator",
        title: "Calculate Indicator",
        toolset: "research",
        scope: Scope::ReadMarket,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Run ANY of the 80+ openalgo.ta indicators over a symbol's historical OHLCV.

    History is fetched (db/api) and the indicator is computed entirely on the
    The Bridge server; only compact results are returned — never the raw OHLCV.

    Args:
        symbol: Stock symbol (e.g., 'RELIANCE', 'NIFTY')
        exchange: Exchange name (NSE, NFO, NSE_INDEX, etc.)
        indicator: ta function name, case-insensitive (e.g., 'rsi','macd','supertrend',
                   'atr','bbands','adx','ema','vwap').
        interval: '1m','3m','5m','10m','15m','30m','1h','D' (default 'D')
        start_date / end_date: YYYY-MM-DD. Optional — when omitted, a lookback window
                   ending today is used.
        params: Extra keyword args for the indicator (e.g., {"period": 14} for rsi;
                {"period": 10, "multiplier": 3} for supertrend;
                {"fast_period": 12, "slow_period": 26, "signal_period": 9} for macd).
        inputs: Ordered list of OHLCV columns to feed the indicator, e.g. ["close"] or
                ["high","low","close"]. Optional — auto-detected for common indicators;
                pass it explicitly if a result errors on inputs.
        bars: Number of most-recent computed rows to return (default 20). The indicator
              is ALWAYS computed server-side over the FULL fetched history; only the last
              `bars` rows (plus latest value and summary stats) are sent back, so the
              payload stays small. Increase only if you explicitly need more rows.
        lookback_bars: Bars of history to load/compute over when dates are omitted
                       (default 252 ≈ one trading year of daily data).
        lookback_days: Alternative calendar-day lookback (e.g., 30 for "last 30 days").
                       Overrides lookback_bars when set.
        source: 'api' (default, broker API) or 'db' (local Historify DuckDB store, which
                supports custom research intervals like 2m/4m/W/M/Q).

    Returns:
        JSON with the latest value(s), summary stats (last/min/max/mean), and a 'data'
        series of the last `bars` rows — all computed server-side. Multi-output indicators
        (macd, bbands, supertrend, stochastic, adx, ichimoku, keltner, donchian) report
        out0, out1, ..."#,
        params: &[
            Param {
                name: "symbol",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "indicator",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "interval",
                ty: Str,
                default: D::Str("D"),
            },
            Param {
                name: "start_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "end_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "params",
                ty: OptObject,
                default: D::Null,
            },
            Param {
                name: "inputs",
                ty: OptStrList,
                default: D::Null,
            },
            Param {
                name: "bars",
                ty: Int,
                default: D::Int(20),
            },
            Param {
                name: "lookback_bars",
                ty: Int,
                default: D::Int(252),
            },
            Param {
                name: "lookback_days",
                ty: OptInt,
                default: D::Null,
            },
            Param {
                name: "source",
                ty: Str,
                default: D::Str("api"),
            },
        ],
    },
    ToolDef {
        name: "get_trend_snapshot",
        title: "Get Trend Snapshot",
        toolset: "research",
        scope: Scope::ReadMarket,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"One-call trend read: SMA(20/50/200), EMA(20/50), Supertrend, ADX/DMI, Ichimoku.

    Args:
        symbol: Stock symbol
        exchange: Exchange name
        interval: Candle interval (default 'D')
        start_date / end_date: YYYY-MM-DD. Optional — default to a lookback window ending today.
        lookback_bars: Bars of history loaded when dates are omitted (default 252, enough for SMA200).
        lookback_days: Alternative calendar-day lookback (e.g., 30). Overrides lookback_bars.

    Returns:
        JSON with latest indicator values and a 'legend' explaining multi-value entries."#,
        params: &[
            Param {
                name: "symbol",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "interval",
                ty: Str,
                default: D::Str("D"),
            },
            Param {
                name: "start_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "end_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "lookback_bars",
                ty: Int,
                default: D::Int(252),
            },
            Param {
                name: "lookback_days",
                ty: OptInt,
                default: D::Null,
            },
            Param {
                name: "source",
                ty: Str,
                default: D::Str("api"),
            },
        ],
    },
    ToolDef {
        name: "get_momentum_snapshot",
        title: "Get Momentum Snapshot",
        toolset: "research",
        scope: Scope::ReadMarket,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"One-call momentum read: RSI(14), MACD, Stochastic, CCI(20), Williams %R(14).

    Args:
        symbol: Stock symbol
        exchange: Exchange name
        interval: Candle interval (default 'D')
        start_date / end_date: YYYY-MM-DD. Optional — default to a lookback window ending today.
        lookback_bars: Bars of history loaded when dates are omitted (default 252).
        lookback_days: Alternative calendar-day lookback (e.g., 30). Overrides lookback_bars.

    Returns:
        JSON with latest values and a 'legend' for multi-value entries."#,
        params: &[
            Param {
                name: "symbol",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "interval",
                ty: Str,
                default: D::Str("D"),
            },
            Param {
                name: "start_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "end_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "lookback_bars",
                ty: Int,
                default: D::Int(252),
            },
            Param {
                name: "lookback_days",
                ty: OptInt,
                default: D::Null,
            },
            Param {
                name: "source",
                ty: Str,
                default: D::Str("api"),
            },
        ],
    },
    ToolDef {
        name: "get_volatility_snapshot",
        title: "Get Volatility Snapshot",
        toolset: "research",
        scope: Scope::ReadMarket,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"One-call volatility read: ATR, NATR, Bollinger Bands (+%B, width), Keltner,
    Donchian, Historical Volatility.

    Args:
        symbol: Stock symbol
        exchange: Exchange name
        interval: Candle interval (default 'D')
        start_date / end_date: YYYY-MM-DD. Optional — default to a lookback window ending today.
        lookback_bars: Bars of history loaded when dates are omitted (default 252).
        lookback_days: Alternative calendar-day lookback (e.g., 30). Overrides lookback_bars.

    Returns:
        JSON with latest values and a 'legend' for multi-value band entries."#,
        params: &[
            Param {
                name: "symbol",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "interval",
                ty: Str,
                default: D::Str("D"),
            },
            Param {
                name: "start_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "end_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "lookback_bars",
                ty: Int,
                default: D::Int(252),
            },
            Param {
                name: "lookback_days",
                ty: OptInt,
                default: D::Null,
            },
            Param {
                name: "source",
                ty: Str,
                default: D::Str("api"),
            },
        ],
    },
    ToolDef {
        name: "get_support_resistance",
        title: "Get Support and Resistance",
        toolset: "research",
        scope: Scope::ReadMarket,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Support/resistance levels: Pivot Points, Donchian channel, and rolling
    highest-high / lowest-low over `period`.

    Args:
        symbol: Stock symbol
        exchange: Exchange name
        interval: Candle interval (default 'D')
        start_date / end_date: YYYY-MM-DD. Optional — default to a lookback window ending today.
        lookback_bars: Bars of history loaded when dates are omitted (default 252).
        lookback_days: Alternative calendar-day lookback (e.g., 30). Overrides lookback_bars.
        period: Lookback window for Donchian / highest / lowest (default 20).

    Returns:
        JSON with latest levels. 'pivot_points' is returned as ta.pivot_points emits it
        (typically [pivot, r1, s1, r2, s2, r3, s3])."#,
        params: &[
            Param {
                name: "symbol",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "interval",
                ty: Str,
                default: D::Str("D"),
            },
            Param {
                name: "start_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "end_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "lookback_bars",
                ty: Int,
                default: D::Int(252),
            },
            Param {
                name: "lookback_days",
                ty: OptInt,
                default: D::Null,
            },
            Param {
                name: "period",
                ty: Int,
                default: D::Int(20),
            },
            Param {
                name: "source",
                ty: Str,
                default: D::Str("api"),
            },
        ],
    },
    ToolDef {
        name: "detect_signals",
        title: "Detect Signals",
        toolset: "research",
        scope: Scope::ReadMarket,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Detect technical signals over a symbol's history using ta crossover/threshold logic.

    Args:
        symbol: Stock symbol
        exchange: Exchange name
        interval: Candle interval (default 'D')
        start_date / end_date: YYYY-MM-DD. Optional — default to the lookback window.
        lookback_bars: Bars loaded when dates omitted (default 252).
        lookback_days: Alternative calendar-day lookback (e.g., 30). Overrides lookback_bars.
        signal_type: One of:
            'ema_cross'      - EMA(fast) crossing EMA(slow)
            'sma_cross'      - SMA(fast) crossing SMA(slow)
            'macd_cross'     - MACD line crossing its signal line
            'supertrend_flip'- Supertrend direction flip
            'rsi_threshold'  - RSI crossing out of oversold(lower) / overbought(upper)
        fast / slow: MA periods for ema_cross / sma_cross
        period: Lookback for rsi_threshold (default 14)
        upper / lower: RSI overbought / oversold levels (default 70 / 30)
        limit: Max number of most-recent signal events to return (default 20)

    Returns:
        JSON with recent events [{timestamp, signal: 'bullish'|'bearish'}] plus current values."#,
        params: &[
            Param {
                name: "symbol",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "interval",
                ty: Str,
                default: D::Str("D"),
            },
            Param {
                name: "start_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "end_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "signal_type",
                ty: Str,
                default: D::Str("ema_cross"),
            },
            Param {
                name: "fast",
                ty: Int,
                default: D::Int(20),
            },
            Param {
                name: "slow",
                ty: Int,
                default: D::Int(50),
            },
            Param {
                name: "period",
                ty: Int,
                default: D::Int(14),
            },
            Param {
                name: "upper",
                ty: Num,
                default: D::Num(70.0),
            },
            Param {
                name: "lower",
                ty: Num,
                default: D::Num(30.0),
            },
            Param {
                name: "limit",
                ty: Int,
                default: D::Int(20),
            },
            Param {
                name: "lookback_bars",
                ty: Int,
                default: D::Int(252),
            },
            Param {
                name: "lookback_days",
                ty: OptInt,
                default: D::Null,
            },
            Param {
                name: "source",
                ty: Str,
                default: D::Str("api"),
            },
        ],
    },
    ToolDef {
        name: "screen_instruments",
        title: "Screen Instruments",
        toolset: "research",
        scope: Scope::ReadMarket,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Scan a watchlist of symbols for a technical condition.

    Note: this fetches history per symbol sequentially — keep the list modest (≤ ~25)
    or use a coarse interval to bound runtime and broker API calls.

    Args:
        symbols: List of {"symbol","exchange"} pairs.
            Example: [{"symbol":"RELIANCE","exchange":"NSE"},{"symbol":"INFY","exchange":"NSE"}]
        interval: Candle interval (default 'D')
        start_date / end_date: YYYY-MM-DD. Optional — default to the lookback window.
        lookback_bars: Bars loaded per symbol when dates omitted (default 252).
        lookback_days: Alternative calendar-day lookback (e.g., 30). Overrides lookback_bars.
        condition: One of:
            'rsi_below' / 'rsi_above'        - RSI(period) vs `value`
            'price_above_sma'/'price_below_sma' - last close vs SMA(period)
            'supertrend_bullish'/'supertrend_bearish' - current Supertrend direction
        value: Threshold for rsi conditions (default 30)
        period: Lookback for rsi / sma (default 14)

    Returns:
        JSON with per-symbol {passed, metric} and a count of matches."#,
        params: &[
            Param {
                name: "symbols",
                ty: StrMapList,
                default: D::Required,
            },
            Param {
                name: "interval",
                ty: Str,
                default: D::Str("D"),
            },
            Param {
                name: "start_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "end_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "condition",
                ty: Str,
                default: D::Str("rsi_below"),
            },
            Param {
                name: "value",
                ty: Num,
                default: D::Num(30.0),
            },
            Param {
                name: "period",
                ty: Int,
                default: D::Int(14),
            },
            Param {
                name: "lookback_bars",
                ty: Int,
                default: D::Int(252),
            },
            Param {
                name: "lookback_days",
                ty: OptInt,
                default: D::Null,
            },
            Param {
                name: "source",
                ty: Str,
                default: D::Str("api"),
            },
        ],
    },
    ToolDef {
        name: "multi_timeframe_analysis",
        title: "Multi-Timeframe Analysis",
        toolset: "research",
        scope: Scope::ReadMarket,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Compute the same indicator across multiple timeframes for confluence analysis.

    Args:
        symbol: Stock symbol
        exchange: Exchange name
        start_date / end_date: YYYY-MM-DD. Optional — default to the lookback window per interval.
        intervals: List of intervals (default ['5m','15m','1h','D'])
        indicator: ta function name (default 'rsi')
        params: Extra keyword args for the indicator (e.g., {"period": 14})
        inputs: Ordered input columns; auto-detected if omitted.
        lookback_bars: Bars loaded per interval when dates omitted (default 252).
        lookback_days: Alternative calendar-day lookback (e.g., 30). Overrides lookback_bars.

    Returns:
        JSON with the latest indicator value (and last_close) per timeframe."#,
        params: &[
            Param {
                name: "symbol",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "start_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "end_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "intervals",
                ty: OptStrList,
                default: D::Null,
            },
            Param {
                name: "indicator",
                ty: Str,
                default: D::Str("rsi"),
            },
            Param {
                name: "params",
                ty: OptObject,
                default: D::Null,
            },
            Param {
                name: "inputs",
                ty: OptStrList,
                default: D::Null,
            },
            Param {
                name: "lookback_bars",
                ty: Int,
                default: D::Int(252),
            },
            Param {
                name: "lookback_days",
                ty: OptInt,
                default: D::Null,
            },
            Param {
                name: "source",
                ty: Str,
                default: D::Str("api"),
            },
        ],
    },
    ToolDef {
        name: "correlation_beta",
        title: "Correlation and Beta",
        toolset: "research",
        scope: Scope::ReadMarket,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Correlation / Beta / Linear-regression slope between two symbols (pairs & hedge research).

    Both symbols' closes are aligned on common timestamps before computing.

    Args:
        symbol1 / exchange1: First instrument (the 'asset')
        symbol2 / exchange2: Second instrument (the 'market'/benchmark)
        interval: Candle interval (default 'D')
        start_date / end_date: YYYY-MM-DD. Optional — default to the lookback window.
        period: Rolling window for correlation/beta/slope (default 20)
        lookback_bars: Bars loaded per symbol when dates omitted (default 252).
        lookback_days: Alternative calendar-day lookback (e.g., 30). Overrides lookback_bars.

    Returns:
        JSON with rolling correlation, rolling beta, LR slope of symbol1, the full-sample
        Pearson correlation, and the number of overlapping bars."#,
        params: &[
            Param {
                name: "symbol1",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange1",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "symbol2",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange2",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "interval",
                ty: Str,
                default: D::Str("D"),
            },
            Param {
                name: "start_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "end_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "period",
                ty: Int,
                default: D::Int(20),
            },
            Param {
                name: "lookback_bars",
                ty: Int,
                default: D::Int(252),
            },
            Param {
                name: "lookback_days",
                ty: OptInt,
                default: D::Null,
            },
            Param {
                name: "source",
                ty: Str,
                default: D::Str("api"),
            },
        ],
    },
];

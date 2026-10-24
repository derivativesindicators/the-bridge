//! The Telegram bot's texts, as the web words them (`_format_*`): book,
//! funds, P&L and quote replies, the help text, and history rows as chart
//! candles.

use super::chart::Candle;
use crate::messaging::format::{
    comma2, comma_int, get_f64, get_str, py_float, py_float_str, py_int, signed2, title,
};
use crate::messaging::openalgo::is_success;
use chrono::TimeZone;
use serde_json::Value;

/// The rule under every heading.
pub(super) const RULE: &str = "━━━━━━━━━━━━━━━";

/// `[+]` / `[-]` / `[=]`.
fn sign_tag(x: f64) -> &'static str {
    if x > 0.0 {
        "[+]"
    } else if x < 0.0 {
        "[-]"
    } else {
        "[=]"
    }
}

fn data(resp: &Value) -> Value {
    resp.get("data").cloned().unwrap_or(Value::Null)
}

fn list_in(v: &Value, key: Option<&str>) -> Vec<Value> {
    let v = match key {
        Some(k) => v.get(k).cloned().unwrap_or(Value::Null),
        None => v.clone(),
    };
    v.as_array().cloned().unwrap_or_default()
}

/// `int(x)` with the web's fallback to the raw value.
fn int_or_raw(v: &Value, key: &str) -> String {
    match v.get(key) {
        None => "0".into(),
        Some(x) => match py_int(x) {
            Some(i) => i.to_string(),
            None => crate::messaging::format::py_str(x),
        },
    }
}

fn quantity_nonzero(p: &Value) -> bool {
    match p.get("quantity") {
        None => false,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Null) => true,
        Some(_) => true,
    }
}

/// `_format_pnl_funds`, shared by `/pnl` and the P&L button.
pub fn format_pnl_funds(resp: &Option<Value>, cs: &str) -> String {
    if !is_success(resp) {
        return "Failed to fetch P&L".into();
    }
    let f = data(resp.as_ref().unwrap_or(&Value::Null));
    let r = get_f64(&f, "m2mrealized", 0.0);
    let u = get_f64(&f, "m2munrealized", 0.0);
    let t = r + u;
    format!(
        "*PROFIT & LOSS*\n{RULE}\n\n{} *Realized P&L*\n└ {cs}{}\n\n{} *Unrealized P&L*\n└ {cs}{}\n\n{} *Total P&L*\n└ {cs}{}",
        sign_tag(r),
        comma2(r),
        sign_tag(u),
        comma2(u),
        sign_tag(t),
        comma2(t)
    )
}

fn order_status_tag(status: &str) -> &'static str {
    match status {
        "complete" => "[OK]",
        "open" => "[OPEN]",
        "rejected" => "[REJECTED]",
        _ => "[OTHER]",
    }
}

fn action_tag(v: &Value) -> &'static str {
    if v.get("action").and_then(Value::as_str) == Some("BUY") {
        "[BUY]"
    } else {
        "[SELL]"
    }
}

fn stat_int(stats: &Value, key: &str) -> i64 {
    stats.get(key).and_then(py_int).unwrap_or(0)
}

/// `/orderbook`.
pub fn format_orderbook_full(resp: &Value, cs: &str) -> String {
    let d = data(resp);
    let orders = list_in(&d, Some("orders"));
    if orders.is_empty() {
        return format!("*ORDERBOOK*\n{RULE}\n\nNo open orders");
    }
    let mut m = format!("*ORDERBOOK*\n{RULE}\n\n");
    for o in orders.iter().take(10) {
        let status = get_str(o, "order_status", "unknown");
        let price_str = match o.get("price").map(py_float).unwrap_or(Some(0.0)) {
            Some(p) => {
                if p == 0.0 && o.get("pricetype").and_then(Value::as_str) == Some("MARKET") {
                    "Market".to_string()
                } else {
                    format!("{cs}{}", py_float_str(p))
                }
            }
            None => format!("{cs}{}", get_str(o, "price", "0")),
        };
        m += &format!(
            "{} *{}* ({})\n{} {} {} @ {}\n├ Type: {}\n├ Product: {}\n├ Status: {}\n├ Time: {}\n",
            order_status_tag(&status),
            get_str(o, "symbol", "N/A"),
            get_str(o, "exchange", "N/A"),
            action_tag(o),
            get_str(o, "action", "N/A"),
            int_or_raw(o, "quantity"),
            price_str,
            get_str(o, "pricetype", "N/A"),
            get_str(o, "product", "N/A"),
            title(&status),
            get_str(o, "timestamp", "N/A"),
        );
        if let Some(t) = o.get("trigger_price").map(py_float).unwrap_or(Some(0.0)) {
            if t > 0.0 {
                m += &format!("├ Trigger: {cs}{}\n", py_float_str(t));
            }
        }
        m += &format!("└ Order ID: `{}`\n\n", get_str(o, "orderid", "N/A"));
    }
    if orders.len() > 10 {
        m += &format!("_... and {} more orders_\n\n", orders.len() - 10);
    }
    let stats = d.get("statistics").cloned().unwrap_or(Value::Null);
    if crate::messaging::format::truthy(Some(&stats)) {
        m += &format!(
            "*Summary*\n├ Total Orders: {}\n├ Open: {}\n├ Completed: {}\n├ Rejected: {}\n├ Buy Orders: {}\n└ Sell Orders: {}",
            orders.len(),
            stat_int(&stats, "total_open_orders"),
            stat_int(&stats, "total_completed_orders"),
            stat_int(&stats, "total_rejected_orders"),
            stat_int(&stats, "total_buy_orders"),
            stat_int(&stats, "total_sell_orders"),
        );
    }
    m
}

/// `/tradebook`.
pub fn format_tradebook_full(resp: &Value, cs: &str) -> String {
    let trades = list_in(resp, Some("data"));
    if trades.is_empty() {
        return format!("*TRADEBOOK*\n{RULE}\n\nNo trades executed today");
    }
    let mut m = format!("*TRADEBOOK*\n{RULE}\n\n");
    let (mut buy, mut sell) = (0.0, 0.0);
    for t in trades.iter().take(10) {
        let value = get_f64(t, "trade_value", 0.0);
        if t.get("action").and_then(Value::as_str) == Some("BUY") {
            buy += value;
        } else {
            sell += value;
        }
        let avg = match t.get("average_price").map(py_float).unwrap_or(Some(0.0)) {
            Some(a) => format!("{cs}{}", comma2(a)),
            None => format!("{cs}{}", get_str(t, "average_price", "0")),
        };
        m += &format!(
            "{} *{}* ({})\n├ {} {} @ {}\n├ Product: {}\n├ Value: {cs}{}\n├ Time: {}\n└ Order ID: `{}`\n\n",
            action_tag(t),
            get_str(t, "symbol", "N/A"),
            get_str(t, "exchange", "N/A"),
            get_str(t, "action", "N/A"),
            int_or_raw(t, "quantity"),
            avg,
            get_str(t, "product", "N/A"),
            comma2(value),
            get_str(t, "timestamp", "N/A"),
            get_str(t, "orderid", "N/A"),
        );
    }
    if trades.len() > 10 {
        m += &format!("_... and {} more trades_\n\n", trades.len() - 10);
    }
    m += &format!(
        "*Summary*\n├ Total Trades: {}\n├ Buy Value: {cs}{}\n└ Sell Value: {cs}{}",
        trades.len(),
        comma2(buy),
        comma2(sell)
    );
    m
}

/// `/positions`.
pub fn format_positions_full(resp: &Value, cs: &str) -> String {
    let positions = list_in(resp, Some("data"));
    if positions.is_empty() {
        return format!("*POSITIONS*\n{RULE}\n\nNo open positions");
    }
    let active: Vec<&Value> = positions.iter().filter(|p| quantity_nonzero(p)).collect();
    if active.is_empty() {
        return format!("*POSITIONS*\n{RULE}\n\nNo active positions");
    }
    let mut m = format!("*POSITIONS*\n{RULE}\n\n");
    let (mut long, mut short) = (0, 0);
    for p in active.iter().take(10) {
        let q = p.get("quantity").and_then(py_int).unwrap_or(0);
        let avg = match p.get("average_price") {
            None => 0.0,
            Some(v) => py_float(v).unwrap_or(0.0),
        };
        let (kind, tag) = if q > 0 {
            long += 1;
            ("LONG", "[LONG]")
        } else {
            short += 1;
            ("SHORT", "[SHORT]")
        };
        m += &format!(
            "{} *{}* ({})\n├ Position: {}\n├ Qty: {} ({})\n",
            tag,
            get_str(p, "symbol", "N/A"),
            get_str(p, "exchange", "N/A"),
            kind,
            q.abs(),
            get_str(p, "product", "N/A"),
        );
        if avg > 0.0 {
            m += &format!("├ Avg Price: {cs}{}\n", comma2(avg));
        }
        m += "\n";
    }
    if active.len() > 10 {
        m += &format!("_... and {} more positions_\n\n", active.len() - 10);
    }
    m += &format!(
        "*Summary*\n├ Active Positions: {}\n├ Long Positions: {}\n└ Short Positions: {}",
        active.len(),
        long,
        short
    );
    m
}

/// `/holdings`.
pub fn format_holdings_full(resp: &Value, cs: &str) -> String {
    let d = data(resp);
    let holdings = list_in(&d, Some("holdings"));
    if holdings.is_empty() {
        return format!("*HOLDINGS*\n{RULE}\n\nNo holdings found");
    }
    let mut m = format!("*HOLDINGS*\n{RULE}\n\n");
    for h in holdings.iter().take(10) {
        let pnl = get_f64(h, "pnl", 0.0);
        let pct = get_f64(h, "pnlpercent", 0.0);
        let q = h
            .get("quantity")
            .map(|v| py_int(v).unwrap_or(0))
            .unwrap_or(0);
        m += &format!(
            "{} *{}* ({})\n├ Product: {}\n├ Qty: {}\n└ P&L: {cs}{} ({}%)\n\n",
            sign_tag(pnl),
            get_str(h, "symbol", "N/A"),
            get_str(h, "exchange", "N/A"),
            get_str(h, "product", "CNC"),
            q,
            comma2(pnl),
            signed2(pct),
        );
    }
    if holdings.len() > 10 {
        m += &format!("_... and {} more holdings_\n\n", holdings.len() - 10);
    }
    let stats = d.get("statistics").cloned().unwrap_or(Value::Null);
    if crate::messaging::format::truthy(Some(&stats)) {
        let tp = get_f64(&stats, "totalprofitandloss", 0.0);
        m += &format!(
            "*Portfolio Summary*\n├ Current Value: {cs}{}\n├ Investment: {cs}{}\n└ {} P&L: {cs}{} ({}%)",
            comma2(get_f64(&stats, "totalholdingvalue", 0.0)),
            comma2(get_f64(&stats, "totalinvvalue", 0.0)),
            sign_tag(tp),
            comma2(tp),
            signed2(get_f64(&stats, "totalpnlpercentage", 0.0)),
        );
    }
    m
}

/// `/funds`.
pub fn format_funds_full(resp: &Value, cs: &str) -> String {
    let f = data(resp);
    let a = get_f64(&f, "availablecash", 0.0);
    let c = get_f64(&f, "collateral", 0.0);
    let u = get_f64(&f, "utiliseddebits", 0.0);
    format!(
        "*FUNDS*\n{RULE}\n\n*Available Cash*\n└ {cs}{}\n\n*Collateral*\n└ {cs}{}\n\n*Utilized Margin*\n└ {cs}{}\n\n*Total Balance*\n└ {cs}{}",
        comma2(a),
        comma2(c),
        comma2(u),
        comma2(a + c)
    )
}

/// The short formats the menu buttons use (`_format_*`).
pub fn format_short(kind: &str, resp: &Option<Value>, cs: &str) -> String {
    let fail = |what: &str| format!("Failed to fetch {}", what);
    if !is_success(resp) {
        return fail(kind);
    }
    let r = resp.as_ref().unwrap_or(&Value::Null);
    match kind {
        "orderbook" => short_orderbook(r, cs),
        "tradebook" => short_tradebook(r, cs),
        "positions" => short_positions(r),
        "holdings" => short_holdings(r, cs),
        "funds" => short_funds(r, cs),
        _ => "Unknown command".into(),
    }
}

/// The orderbook button: the first ten orders.
fn short_orderbook(r: &Value, cs: &str) -> String {
    let orders = list_in(&data(r), Some("orders"));
    if orders.is_empty() {
        return format!("*ORDERBOOK*\n{RULE}\n\nNo open orders");
    }
    let mut m = format!("*ORDERBOOK*\n{RULE}\n\n");
    for o in orders.iter().take(10) {
        let status = get_str(o, "order_status", "unknown");
        let price = match o.get("price").map(py_float).unwrap_or(Some(0.0)) {
            Some(p) => {
                if p == 0.0 {
                    "Market".to_string()
                } else {
                    format!("{cs}{}", py_float_str(p))
                }
            }
            None => format!("{cs}{}", get_str(o, "price", "0")),
        };
        m += &format!(
            "{} *{}* ({})\n{} {} {} @ {}\n└ Status: {}\n\n",
            order_status_tag(&status),
            get_str(o, "symbol", "N/A"),
            get_str(o, "exchange", "N/A"),
            action_tag(o),
            get_str(o, "action", "N/A"),
            int_or_raw(o, "quantity"),
            price,
            title(&status)
        );
    }
    if orders.len() > 10 {
        m += &format!("_... and {} more orders_", orders.len() - 10);
    }
    m
}

/// The tradebook button: the first ten of today's trades.
fn short_tradebook(r: &Value, cs: &str) -> String {
    let trades = list_in(r, Some("data"));
    if trades.is_empty() {
        return format!("*TRADEBOOK*\n{RULE}\n\nNo trades executed today");
    }
    let mut m = format!("*TRADEBOOK*\n{RULE}\n\n");
    for t in trades.iter().take(10) {
        let avg = match t.get("average_price").map(py_float).unwrap_or(Some(0.0)) {
            Some(a) => format!("{cs}{}", comma2(a)),
            None => format!("{cs}{}", get_str(t, "average_price", "0")),
        };
        m += &format!(
            "{} *{}* ({})\n├ {} {} @ {}\n└ Time: {}\n\n",
            action_tag(t),
            get_str(t, "symbol", "N/A"),
            get_str(t, "exchange", "N/A"),
            get_str(t, "action", "N/A"),
            int_or_raw(t, "quantity"),
            avg,
            get_str(t, "timestamp", "N/A")
        );
    }
    if trades.len() > 10 {
        m += &format!("_... and {} more trades_", trades.len() - 10);
    }
    m
}

/// The positions button: the first ten open positions.
fn short_positions(r: &Value) -> String {
    let positions = list_in(r, Some("data"));
    let active: Vec<&Value> = positions.iter().filter(|p| quantity_nonzero(p)).collect();
    if active.is_empty() {
        return format!("*POSITIONS*\n{RULE}\n\nNo active positions");
    }
    let mut m = format!("*POSITIONS*\n{RULE}\n\n");
    for p in active.iter().take(10) {
        let q = p.get("quantity").and_then(py_int).unwrap_or(0);
        let (tag, kind) = if q > 0 {
            ("[LONG]", "LONG")
        } else {
            ("[SHORT]", "SHORT")
        };
        m += &format!(
            "{} *{}* ({})\n├ {}\n└ Qty: {}\n\n",
            tag,
            get_str(p, "symbol", "N/A"),
            get_str(p, "exchange", "N/A"),
            kind,
            q.abs()
        );
    }
    if active.len() > 10 {
        m += &format!("_... and {} more positions_", active.len() - 10);
    }
    m
}

/// The holdings button: the first ten holdings with their P&L.
fn short_holdings(r: &Value, cs: &str) -> String {
    let holdings = list_in(&data(r), Some("holdings"));
    if holdings.is_empty() {
        return format!("*HOLDINGS*\n{RULE}\n\nNo holdings found");
    }
    let mut m = format!("*HOLDINGS*\n{RULE}\n\n");
    for h in holdings.iter().take(10) {
        let (pnl, pct) = match (
            h.get("pnl").map(py_float).unwrap_or(Some(0.0)),
            h.get("pnlpercent").map(py_float).unwrap_or(Some(0.0)),
        ) {
            (Some(a), Some(b)) => (a, b),
            _ => (0.0, 0.0),
        };
        m += &format!(
            "{} *{}*\n└ P&L: {cs}{} ({}%)\n\n",
            sign_tag(pnl),
            get_str(h, "symbol", "N/A"),
            comma2(pnl),
            signed2(pct)
        );
    }
    if holdings.len() > 10 {
        m += &format!("_... and {} more holdings_", holdings.len() - 10);
    }
    m
}

/// The funds button: cash, collateral, utilised and total.
fn short_funds(r: &Value, cs: &str) -> String {
    let f = data(r);
    let vals = (
        f.get("availablecash").map(py_float).unwrap_or(Some(0.0)),
        f.get("collateral").map(py_float).unwrap_or(Some(0.0)),
        f.get("utiliseddebits").map(py_float).unwrap_or(Some(0.0)),
    );
    let (a, c, u) = match vals {
        (Some(a), Some(c), Some(u)) => (a, c, u),
        _ => (0.0, 0.0, 0.0),
    };
    format!(
        "*FUNDS*\n{RULE}\n\nAvailable: {cs}{}\nCollateral: {cs}{}\nUtilized: {cs}{}\nTotal: {cs}{}",
        comma2(a),
        comma2(c),
        comma2(u),
        comma2(a + c)
    )
}

/// `/quote`.
pub fn format_quote(symbol: &str, resp: &Value, cs: &str) -> String {
    let q = data(resp);
    let ltp = get_f64(&q, "ltp", 0.0);
    let prev = get_f64(&q, "prev_close", ltp);
    let change = ltp - prev;
    let pct = if prev > 0.0 {
        change / prev * 100.0
    } else {
        0.0
    };
    let vol = q.get("volume").map(|v| py_int(v).unwrap_or(0)).unwrap_or(0);
    format!(
        "*{symbol}*\n{RULE}\n\n{} Price: {cs}{}\n├ Change: {cs}{} ({}%)\n├ Open: {cs}{}\n├ High: {cs}{}\n├ Low: {cs}{}\n├ Prev Close: {cs}{}\n└ Volume: {}",
        sign_tag(change),
        comma2(ltp),
        signed2(change),
        signed2(pct),
        comma2(get_f64(&q, "open", 0.0)),
        comma2(get_f64(&q, "high", 0.0)),
        comma2(get_f64(&q, "low", 0.0)),
        comma2(prev),
        comma_int(vol)
    )
}

pub const HELP_TEXT: &str = "
*Available Commands:*

*Account Management:*
/link `<api_key> <host_url>` - Link your The Bridge account
/unlink - Unlink your account
/status - Check connection status

*Trading Information:*
/orderbook - View open orders
/tradebook - View executed trades
/positions - View current positions
/holdings - View holdings
/funds - View account funds
/pnl - View P&L (realized & unrealized)
/quote `<symbol> [exchange]` - Get stock quote

*Charts:*
/chart `<symbol> [exchange] [type] [interval] [days]`
  • type: intraday or daily (default: both)
  • interval: 1m, 5m, 15m, 30m, 1h, D (default: 5m for intraday, D for daily)
  • days: number of days (default: 5 for intraday, 252 for daily)

*Remote Actions:*
/closeall - Close all open positions (with confirmation)
/mode - View or toggle trading mode (Live / Analyze)

*Navigation:*
/menu - Show interactive menu
/help - Show this help message

*Examples:*
`/quote RELIANCE`
`/quote NIFTY NSE_INDEX`
`/chart RELIANCE`
`/chart RELIANCE NSE intraday 15m 10`
`/chart NIFTY NSE_INDEX daily D 100`
";

/// Turn history rows into candles in IST (the SDK's DataFrame index).
pub fn candles_from_history(resp: &Value) -> Vec<Candle> {
    let ist = crate::clock::ist_offset();
    list_in(resp, Some("data"))
        .iter()
        .filter_map(|r| {
            let ts = r.get("timestamp").and_then(py_float)?;
            let time = ist.timestamp_opt(ts as i64, 0).single()?;
            Some(Candle {
                time,
                open: r.get("open").and_then(py_float)?,
                high: r.get("high").and_then(py_float)?,
                low: r.get("low").and_then(py_float)?,
                close: r.get("close").and_then(py_float)?,
                volume: r.get("volume").and_then(py_float).unwrap_or(0.0),
            })
        })
        .collect()
}

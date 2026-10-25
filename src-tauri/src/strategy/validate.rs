//! Strategy configuration validation (web `blueprints/strategy_module.py`,
//! `validate_strategy_config`). The store has no opinion on a leg's shape
//! (legs are JSON), so this is the only place a bad payload is refused.
//!
//! * Unknown fields are refused, never dropped: a misspelt `overall_sl_mtm`
//!   must not save as a strategy with no stop.
//! * Enums are matched case-insensitively and returned canonically, so the
//!   validator is idempotent (a PATCH re-validates the merged config).
//! * A loss threshold is entered positive and applied negative.
//! * A strike keeps its fractional part.

use super::resolver::is_derivative_exchange;
use super::store::{DIRECTIONS, RUN_MODES, STRATEGY_KINDS, STRATEGY_TYPES, UPDATABLE_FIELDS};
use crate::brokers::common::symbols::SymbolGeneration;
use serde_json::{json, Map, Value};

mod legs;
pub use legs::is_resolved_signal_leg;

use legs::validate_legs;

pub const PRODUCTS: &[&str] = &["CNC", "NRML", "MIS"];
/// MARKET only: no leg carries a price, and exits are MARKET regardless.
pub const PRICETYPES: &[&str] = &["MARKET"];
pub const UNDERLYING_EXCHANGES: &[&str] = &[
    "NSE",
    "BSE",
    "NFO",
    "BFO",
    "CDS",
    "BCD",
    "MCX",
    "NCDEX",
    "NCO",
    "NSE_INDEX",
    "BSE_INDEX",
];
pub const UNIVERSE_TABS: &[&str] = &["weekly_monthly", "monthly_only", "stocks_fno", "mcx"];
pub const LEG_SEGMENTS: &[&str] = &["options", "futures", "cash"];
pub const LEG_POSITIONS: &[&str] = &["B", "S"];
pub const LEG_OPTION_TYPES: &[&str] = &["CE", "PE"];
pub const LEG_STRIKE_MODES: &[&str] = &["atm", "strike"];
pub const LEG_EXPIRIES: &[&str] = &[
    "weekly",
    "next_week",
    "monthly",
    "next_month",
    "current",
    "next",
];
pub const LOCK_PROFIT_MODES: &[&str] = &["lock", "lock_and_trail"];
pub const RISK_UNITS: &[&str] = &["points", "percent"];
pub const SCHEDULER_DAYS: &[&str] = &["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];
pub const SIGNAL_LEG_EXCHANGES: &[&str] = &[
    "NSE", "BSE", "NFO", "BFO", "MCX", "CDS", "BCD", "NCDEX", "NCO",
];
pub const LEG_SIDES: &[&str] = &["long", "short", "both"];
pub const QTY_MODES: &[&str] = &["lots", "units"];

const LEG_FIELDS: &[&str] = &[
    "id",
    "segment",
    "position",
    "lots",
    "option_type",
    "strike_mode",
    "atm_offset",
    "strike",
    "expiry",
    "sl_pts",
    "target_pts",
    "trail",
    "risk_unit",
];
const SIGNAL_LEG_FIELDS: &[&str] = &[
    "id",
    "symbol",
    "exchange",
    "side",
    "qty",
    "qty_mode",
    "segment",
    "expiry",
    "sl_pts",
    "target_pts",
    "trail",
    "risk_unit",
];
const TRAIL_FIELDS: &[&str] = &["x", "y"];
const LOCK_PROFIT_FIELDS: &[&str] = &["mode", "if_profit_reaches", "lock_profit", "trail_step"];
const SCHEDULER_FIELDS: &[&str] = &[
    "enabled",
    "days",
    "start_time",
    "auto_stop_time",
    "default_mode",
];

pub const MIN_LEGS: usize = 1;
pub const MAX_LEGS: usize = 10;
pub const MAX_LOTS: i64 = 50;
pub const MAX_CASH_QUANTITY: i64 = 1_000_000;
pub const MAX_SIGNAL_QTY: i64 = 1_000_000;
pub const MAX_SIGNAL_LOTS: i64 = 10_000;
pub const MAX_NAME_LENGTH: usize = 200;
pub const MAX_UNDERLYING_LENGTH: usize = 50;
pub const MAX_IP_ALLOWLIST: usize = 20;

/// `CONFIG_FIELDS`: the store's PATCH allowlist plus `strategy_kind`.
pub fn config_fields() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = UPDATABLE_FIELDS.to_vec();
    v.push("strategy_kind");
    v.sort();
    v
}

type R<T> = Result<T, String>;

fn mapping<'a>(v: &'a Value, label: &str) -> R<&'a Map<String, Value>> {
    v.as_object()
        .ok_or_else(|| format!("{} must be a JSON object", label))
}

fn reject_unknown(m: &Map<String, Value>, allowed: &[&str], label: &str) -> R<()> {
    let mut extra: Vec<&String> = m
        .keys()
        .filter(|k| !allowed.contains(&k.as_str()))
        .collect();
    if extra.is_empty() {
        return Ok(());
    }
    extra.sort();
    let mut allowed_sorted = allowed.to_vec();
    allowed_sorted.sort();
    Err(format!(
        "{} does not accept {}. Accepted fields: {}",
        label,
        extra
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        allowed_sorted.join(", ")
    ))
}

fn get<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a Value> {
    m.get(k).filter(|v| !v.is_null())
}

fn required<'a>(m: &'a Map<String, Value>, k: &str, label: &str) -> R<&'a Value> {
    get(m, k).ok_or_else(|| {
        if label.is_empty() {
            format!("{} is required", k)
        } else {
            format!("{}.{} is required", label, k)
        }
    })
}

fn text(v: &Value, label: &str, max: usize) -> R<String> {
    let s = v
        .as_str()
        .ok_or_else(|| format!("{} must be text", label))?
        .trim()
        .to_string();
    if s.is_empty() {
        return Err(format!("{} is required", label));
    }
    let n = s.chars().count();
    if n > max {
        return Err(format!(
            "{} must be at most {} characters, got {}",
            label, max, n
        ));
    }
    Ok(s)
}

fn py_repr(v: &Value) -> String {
    match v {
        Value::String(s) => format!("'{}'", s),
        Value::Null => "None".into(),
        Value::Bool(b) => {
            if *b {
                "True".into()
            } else {
                "False".into()
            }
        }
        other => other.to_string(),
    }
}

fn choice(v: &Value, allowed: &[&'static str], label: &str) -> R<&'static str> {
    let Some(s) = v.as_str() else {
        return Err(format!("{} must be one of: {}", label, allowed.join(", ")));
    };
    let t = s.trim();
    allowed
        .iter()
        .find(|o| o.eq_ignore_ascii_case(t))
        .copied()
        .ok_or_else(|| {
            format!(
                "{} must be one of: {}. Got {}",
                label,
                allowed.join(", "),
                py_repr(v)
            )
        })
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{}", n)
    }
}

/// A finite number, type preserved (int stays int, float stays float).
fn number(v: &Value, label: &str, min: Option<f64>, max: Option<f64>, gt: Option<f64>) -> R<Value> {
    let out = match v {
        Value::Bool(_) => return Err(format!("{} must be a number", label)),
        Value::String(s) => match s.trim().parse::<f64>() {
            Ok(f) if f.is_finite() => json!(f),
            _ => return Err(format!("{} must be a number, got {}", label, py_repr(v))),
        },
        Value::Number(n) => {
            if n.as_f64().map(f64::is_finite) != Some(true) {
                return Err(format!("{} must be a number", label));
            }
            v.clone()
        }
        _ => return Err(format!("{} must be a number", label)),
    };
    let f = out.as_f64().unwrap_or(0.0);
    if let Some(m) = min {
        if f < m {
            return Err(format!(
                "{} must be {} or more, got {}",
                label,
                fmt_num(m),
                fmt_num(f)
            ));
        }
    }
    if let Some(g) = gt {
        if f <= g {
            return Err(format!(
                "{} must be greater than {}, got {}",
                label,
                fmt_num(g),
                fmt_num(f)
            ));
        }
    }
    if let Some(m) = max {
        if f > m {
            return Err(format!(
                "{} must be {} or less, got {}",
                label,
                fmt_num(m),
                fmt_num(f)
            ));
        }
    }
    Ok(out)
}

fn integer(v: &Value, label: &str, min: i64, max: i64) -> R<i64> {
    let n = match v {
        Value::Bool(_) => return Err(format!("{} must be a whole number", label)),
        Value::String(s) => s
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{} must be a whole number, got {}", label, py_repr(v)))?,
        Value::Number(n) => match n.as_i64() {
            Some(i) => i,
            None => {
                let f = n.as_f64().unwrap_or(f64::NAN);
                if f.is_finite() && f.fract() == 0.0 {
                    f as i64
                } else {
                    return Err(format!("{} must be a whole number, got {}", label, n));
                }
            }
        },
        _ => return Err(format!("{} must be a whole number", label)),
    };
    if n < min || n > max {
        return Err(format!(
            "{} must be between {} and {}, got {}",
            label, min, max, n
        ));
    }
    Ok(n)
}

fn boolean(v: &Value, label: &str) -> R<bool> {
    v.as_bool()
        .ok_or_else(|| format!("{} must be true or false", label))
}

/// `HH:MM` 24-hour, returned canonically (`09:20`).
pub fn hhmm(v: &Value, label: &str) -> R<String> {
    let bad = || format!("{} must be a HH:MM 24-hour time, for example 09:20", label);
    let s = v.as_str().ok_or_else(bad)?.trim();
    let (h, m) = s
        .split_once(':')
        .filter(|(h, m)| (1..=2).contains(&h.len()) && m.len() == 2)
        .ok_or_else(|| format!("{}. Got {}", bad(), py_repr(v)))?;
    let (h, m): (u32, u32) = match (h.parse(), m.parse()) {
        (Ok(h), Ok(m)) => (h, m),
        _ => return Err(format!("{}. Got {}", bad(), py_repr(v))),
    };
    if h > 23 || m > 59 {
        return Err(format!(
            "{} is not a valid time of day: {}",
            label,
            py_repr(v)
        ));
    }
    Ok(format!("{:02}:{:02}", h, m))
}

fn loss_amount(v: Option<&Value>, label: &str) -> R<Value> {
    let Some(v) = v else {
        return Ok(Value::Null);
    };
    let n = number(v, label, None, None, None)?;
    if n.as_f64().unwrap_or(0.0) < 0.0 {
        return Err(format!(
            "{} is entered as a positive amount and applied as a negative threshold, so it cannot be negative. Enter 5000 to stop at a loss of 5000.",
            label
        ));
    }
    Ok(n)
}

fn risk_max(unit: &str) -> Option<f64> {
    (unit == "percent").then_some(100.0)
}

fn trail(v: Option<&Value>, label: &str, unit: &str) -> R<Option<Value>> {
    let Some(v) = v else {
        return Ok(None);
    };
    let m = mapping(v, label)?;
    reject_unknown(m, TRAIL_FIELDS, label)?;
    Ok(Some(json!({
        "x": number(required(m, "x", label)?, &format!("{}.x", label), Some(0.0), risk_max(unit), None)?,
        "y": number(required(m, "y", label)?, &format!("{}.y", label), Some(0.0), risk_max(unit), None)?,
    })))
}

fn risk_fields(
    leg: &Map<String, Value>,
    clean: &mut Map<String, Value>,
    label: &str,
    trail_label: &str,
) -> R<()> {
    let unit = choice(
        get(leg, "risk_unit").unwrap_or(&json!("points")),
        RISK_UNITS,
        &format!("{}.risk_unit", label),
    )?;
    for k in ["sl_pts", "target_pts"] {
        if let Some(v) = get(leg, k) {
            clean.insert(
                k.into(),
                number(
                    v,
                    &format!("{}.{}", label, k),
                    Some(0.0),
                    risk_max(unit),
                    None,
                )?,
            );
        }
    }
    if let Some(t) = trail(get(leg, "trail"), trail_label, unit)? {
        clean.insert("trail".into(), t);
    }
    clean.insert("risk_unit".into(), json!(unit));
    Ok(())
}

fn lock_profit(v: Option<&Value>) -> R<Value> {
    let Some(v) = v else {
        return Ok(Value::Null);
    };
    let label = "lock_profit";
    let m = mapping(v, label)?;
    reject_unknown(m, LOCK_PROFIT_FIELDS, label)?;
    let mode = choice(
        required(m, "mode", label)?,
        LOCK_PROFIT_MODES,
        "lock_profit.mode",
    )?;
    let reaches = number(
        required(m, "if_profit_reaches", label)?,
        "lock_profit.if_profit_reaches",
        None,
        None,
        Some(0.0),
    )?;
    let locked = number(
        required(m, "lock_profit", label)?,
        "lock_profit.lock_profit",
        Some(0.0),
        None,
        None,
    )?;
    if locked.as_f64() > reaches.as_f64() {
        return Err(
            "lock_profit.lock_profit cannot be more than lock_profit.if_profit_reaches: the floor would be above the profit that arms it"
                .into(),
        );
    }
    let mut c = Map::new();
    c.insert("mode".into(), json!(mode));
    c.insert("if_profit_reaches".into(), reaches);
    c.insert("lock_profit".into(), locked);
    if mode == "lock_and_trail" {
        let step = get(m, "trail_step")
            .ok_or("lock_profit.trail_step is required when mode is 'lock_and_trail'")?;
        c.insert(
            "trail_step".into(),
            number(step, "lock_profit.trail_step", None, None, Some(0.0))?,
        );
    } else if let Some(step) = get(m, "trail_step") {
        c.insert(
            "trail_step".into(),
            number(step, "lock_profit.trail_step", None, None, Some(0.0))?,
        );
    }
    Ok(Value::Object(c))
}

fn scheduler(v: Option<&Value>) -> R<Value> {
    let Some(v) = v else {
        return Ok(Value::Null);
    };
    let label = "scheduler";
    let m = mapping(v, label)?;
    reject_unknown(m, SCHEDULER_FIELDS, label)?;
    let enabled = boolean(
        get(m, "enabled").unwrap_or(&json!(false)),
        "scheduler.enabled",
    )?;
    let raw_days = get(m, "days").cloned().unwrap_or(json!([]));
    let list = raw_days
        .as_array()
        .ok_or("scheduler.days must be a list of days, MON to SUN")?;
    let mut days: Vec<&'static str> = Vec::new();
    for (i, d) in list.iter().enumerate() {
        let c = choice(d, SCHEDULER_DAYS, &format!("scheduler.days[{}]", i))?;
        if days.contains(&c) {
            return Err(format!("scheduler.days lists {} more than once", c));
        }
        days.push(c);
    }
    if enabled && days.is_empty() {
        return Err("scheduler.days needs at least one day when the scheduler is enabled".into());
    }
    let (start_raw, stop_raw) = if enabled {
        (
            Some(required(m, "start_time", label)?),
            Some(required(m, "auto_stop_time", label)?),
        )
    } else {
        (get(m, "start_time"), get(m, "auto_stop_time"))
    };
    let start = start_raw
        .map(|v| hhmm(v, "scheduler.start_time"))
        .transpose()?;
    let stop = stop_raw
        .map(|v| hhmm(v, "scheduler.auto_stop_time"))
        .transpose()?;
    if let (Some(a), Some(b)) = (&start, &stop) {
        if a >= b {
            return Err(
                "scheduler.start_time must be earlier than scheduler.auto_stop_time".into(),
            );
        }
    }
    days.sort_by_key(|d| SCHEDULER_DAYS.iter().position(|x| x == d));
    Ok(json!({
        "enabled": enabled,
        "days": days,
        "start_time": start,
        "auto_stop_time": stop,
        "default_mode": choice(get(m, "default_mode").unwrap_or(&json!("sandbox")), RUN_MODES, "scheduler.default_mode")?,
    }))
}

fn ip_allowlist(v: Option<&Value>) -> R<Value> {
    let Some(v) = v else {
        return Ok(Value::Null);
    };
    let label = "webhook_ip_allowlist";
    let list = v
        .as_array()
        .ok_or_else(|| format!("{} must be a list of IP addresses or CIDR ranges", label))?;
    if list.len() > MAX_IP_ALLOWLIST {
        return Err(format!(
            "{} takes at most {} entries, got {}",
            label,
            MAX_IP_ALLOWLIST,
            list.len()
        ));
    }
    let mut out = Vec::new();
    for (i, e) in list.iter().enumerate() {
        let t = e
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("{}[{}] must be an IP address or CIDR range", label, i))?;
        if super::webhook::parse_network(t).is_none() {
            return Err(format!(
                "{}[{}] is not a valid IP address or CIDR range: '{}'",
                label, i, t
            ));
        }
        out.push(json!(t));
    }
    Ok(Value::Array(out))
}

fn tab_for_legs(raw: &Map<String, Value>, legs: &[Value]) -> &'static str {
    if legs.iter().any(|l| l["segment"] == "cash") {
        return "stocks_fno";
    }
    let ux = raw
        .get("underlying_exchange")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_uppercase();
    if ["MCX", "NCDEX", "NCO"].contains(&ux.as_str()) {
        return "mcx";
    }
    if legs.iter().any(|l| {
        matches!(
            l["expiry"]
                .as_str()
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str(),
            "weekly" | "next_week"
        )
    }) {
        "weekly_monthly"
    } else {
        "monthly_only"
    }
}

fn tab_segments(tab: &str) -> &'static [&'static str] {
    match tab {
        "stocks_fno" => &["cash", "futures", "options"],
        _ => &["futures", "options"],
    }
}

/// The run-wide risk and schedule settings: the overall stop and target,
/// the profit lock, trail-to-entry, the scheduler, the daily loss limit
/// and the webhook allowlist.
fn risk_settings(raw: &Map<String, Value>, c: &mut Map<String, Value>) -> R<()> {
    c.insert(
        "overall_sl_mtm".into(),
        loss_amount(get(raw, "overall_sl_mtm"), "overall_sl_mtm")?,
    );
    c.insert(
        "overall_target_mtm".into(),
        match get(raw, "overall_target_mtm") {
            Some(v) => number(v, "overall_target_mtm", Some(0.0), None, None)?,
            None => Value::Null,
        },
    );
    c.insert("lock_profit".into(), lock_profit(get(raw, "lock_profit"))?);
    c.insert(
        "trail_sl_to_entry".into(),
        json!(boolean(
            get(raw, "trail_sl_to_entry").unwrap_or(&json!(false)),
            "trail_sl_to_entry"
        )?),
    );
    c.insert("scheduler".into(), scheduler(get(raw, "scheduler"))?);
    c.insert(
        "daily_loss_limit_inr".into(),
        loss_amount(get(raw, "daily_loss_limit_inr"), "daily_loss_limit_inr")?,
    );
    c.insert(
        "webhook_ip_allowlist".into(),
        ip_allowlist(get(raw, "webhook_ip_allowlist"))?,
    );
    Ok(())
}

/// The entry and exit times, required for an intraday strategy, and the
/// entry before the exit.
fn session_times(raw: &Map<String, Value>, stype: &str) -> R<(Option<String>, Option<String>)> {
    let entry = get(raw, "entry_time");
    let exit = get(raw, "exit_time");
    if stype == "intraday" {
        if entry.is_none() {
            return Err("entry_time is required for an intraday strategy".into());
        }
        if exit.is_none() {
            return Err("exit_time is required for an intraday strategy".into());
        }
    }
    let entry = entry.map(|v| hhmm(v, "entry_time")).transpose()?;
    let exit = exit.map(|v| hhmm(v, "exit_time")).transpose()?;
    if let (Some(a), Some(b)) = (&entry, &exit) {
        if a >= b {
            return Err("entry_time must be earlier than exit_time".into());
        }
    }
    Ok((entry, exit))
}

/// The rules across fields: a signal leg's side must be one its
/// direction acts on, every leg's segment must be one its universe tab
/// offers, a signal's cash and futures legs must sit on matching
/// exchanges, and a batch's short cash leg must be intraday.
fn cross_field_rules(
    kind: &str,
    direction: &str,
    (product, tab): (&str, &str),
    legs: &[Value],
) -> R<()> {
    if kind == "signal" {
        let accepted: &[&str] = match direction {
            "long_only" => &["long", "both"],
            "short_only" => &["short", "both"],
            _ => &["long", "short", "both"],
        };
        for (i, leg) in legs.iter().enumerate() {
            let side = leg["side"].as_str().unwrap_or("both");
            if !accepted.contains(&side) {
                let mut a = accepted.to_vec();
                a.sort();
                return Err(format!(
                    "legs[{}].side is '{}', which a '{}' strategy never acts on. Use {}, or change the strategy direction.",
                    i,
                    side,
                    direction,
                    a.join(" or ")
                ));
            }
        }
    }
    let allowed = tab_segments(tab);
    for (i, leg) in legs.iter().enumerate() {
        if let Some(seg) = leg["segment"].as_str() {
            if !allowed.contains(&seg) {
                return Err(format!(
                    "legs[{}].segment is '{}', which the '{}' universe does not offer. That tab trades {}.",
                    i,
                    seg,
                    tab,
                    allowed.join(" and ")
                ));
            }
        }
    }
    if kind == "signal" {
        for (i, leg) in legs.iter().enumerate() {
            if legs::is_resolved_signal_leg(leg) {
                continue;
            }
            let seg = leg["segment"].as_str().unwrap_or("");
            let ex = leg["exchange"].as_str().unwrap_or("");
            if seg == "cash" && is_derivative_exchange(ex) {
                return Err(format!(
                    "legs[{}] is a cash leg on {}, which lists derivatives. Use NSE or BSE for cash, or set the segment to 'futures'.",
                    i, ex
                ));
            }
            if seg == "futures" && !is_derivative_exchange(ex) {
                return Err(format!(
                    "legs[{}] is a futures leg on {}, which lists cash. Use a derivative exchange, or set the segment to 'cash'.",
                    i, ex
                ));
            }
        }
    } else if product != "MIS" {
        for (i, leg) in legs.iter().enumerate() {
            if leg["segment"] == "cash" && leg["position"] == "S" {
                return Err(format!(
                    "legs[{}] sells cash short, but product '{}' carries the position. Cash cannot be held short overnight. Use MIS for an intraday short, or make the leg long.",
                    i, product
                ));
            }
        }
    }
    Ok(())
}

/// Validate a whole strategy configuration (create, and a merged PATCH).
pub fn validate_strategy_config(payload: &Value, g: &SymbolGeneration) -> R<Value> {
    let raw = mapping(payload, "The request body")?;
    let fields = config_fields();
    reject_unknown(raw, &fields, "The request")?;
    let kind = choice(
        get(raw, "strategy_kind").unwrap_or(&json!("batch")),
        STRATEGY_KINDS,
        "strategy_kind",
    )?;
    let legs = validate_legs(required(raw, "legs", "")?, kind, g)?;
    let mut c = Map::new();
    c.insert(
        "name".into(),
        json!(text(required(raw, "name", "")?, "name", MAX_NAME_LENGTH)?),
    );
    c.insert("strategy_kind".into(), json!(kind));
    c.insert(
        "direction".into(),
        json!(choice(
            get(raw, "direction").unwrap_or(&json!("both")),
            DIRECTIONS,
            "direction"
        )?),
    );
    let tab = match get(raw, "universe_tab") {
        Some(v) => choice(v, UNIVERSE_TABS, "universe_tab")?,
        None => tab_for_legs(raw, &legs),
    };
    c.insert("universe_tab".into(), json!(tab));
    c.insert(
        "underlying".into(),
        json!(text(
            required(raw, "underlying", "")?,
            "underlying",
            MAX_UNDERLYING_LENGTH
        )?
        .to_ascii_uppercase()),
    );
    c.insert(
        "underlying_exchange".into(),
        json!(choice(
            required(raw, "underlying_exchange", "")?,
            UNDERLYING_EXCHANGES,
            "underlying_exchange"
        )?),
    );
    let stype = choice(
        get(raw, "strategy_type").unwrap_or(&json!("intraday")),
        STRATEGY_TYPES,
        "strategy_type",
    )?;
    c.insert("strategy_type".into(), json!(stype));
    let product = choice(
        get(raw, "product").unwrap_or(&json!("NRML")),
        PRODUCTS,
        "product",
    )?;
    c.insert("product".into(), json!(product));
    c.insert(
        "pricetype".into(),
        json!(choice(
            get(raw, "pricetype").unwrap_or(&json!("MARKET")),
            PRICETYPES,
            "pricetype"
        )?),
    );
    c.insert("legs".into(), Value::Array(legs.clone()));
    risk_settings(raw, &mut c)?;

    let (entry, exit) = session_times(raw, stype)?;
    c.insert("entry_time".into(), json!(entry));
    c.insert("exit_time".into(), json!(exit));

    cross_field_rules(
        kind,
        c["direction"].as_str().unwrap_or("both"),
        (product, tab),
        &legs,
    )?;
    Ok(Value::Object(c))
}

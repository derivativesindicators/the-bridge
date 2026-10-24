//! Argument helpers: reading and checking the canonical arguments
//! `normalize` produced, and the indicator inputs and summaries.

use super::history::{opt, strict_date, Frame, Interval};
use super::limits::{limit_error, MAX_BARS, MAX_DAYS};
use crate::mcp::envelope::error;
use crate::mcp::ta::{self, Out, PyErr};
use serde_json::{json, Map, Value};

pub(super) fn s(a: &Map<String, Value>, k: &str) -> Option<String> {
    a.get(k).and_then(Value::as_str).map(str::to_string)
}

pub(super) fn i(a: &Map<String, Value>, k: &str) -> Option<i64> {
    a.get(k).and_then(Value::as_i64)
}

pub(super) fn f(a: &Map<String, Value>, k: &str) -> Option<f64> {
    a.get(k).and_then(Value::as_f64)
}

pub(super) fn lookback(a: &Map<String, Value>) -> i64 {
    i(a, "lookback_bars").unwrap_or(252)
}

/// Integer arguments and their allowed range.
const INT_ARGS: &[(&str, i64)] = &[
    ("bars", MAX_BARS),
    ("lookback_bars", MAX_BARS),
    ("limit", MAX_BARS),
    ("lookback_days", MAX_DAYS),
    ("period", 1_000_000),
    ("fast", 1_000_000),
    ("slow", 1_000_000),
];
/// Number arguments and their allowed magnitude.
const NUM_ARGS: &[(&str, f64)] = &[("value", 1e12), ("upper", 1e12), ("lower", 1e12)];

/// A whole number as given: a JSON integer, an integral float, or a
/// string of digits with an optional leading minus (no spaces, no
/// exponent, at most 12 characters). `None` when it is not one.
pub(super) fn strict_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite() && f.fract() == 0.0 && f.abs() <= 1e15)
                .map(|f| f as i64)
        }),
        Value::String(t) => {
            let digits = t.strip_prefix('-').unwrap_or(t);
            if t.len() > 12 || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            t.parse().ok()
        }
        _ => None,
    }
}

fn strict_num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64().filter(|f| f.is_finite()),
        Value::String(t) if t.len() <= 32 && t.trim() == t => {
            t.parse::<f64>().ok().filter(|f| f.is_finite())
        }
        _ => None,
    }
}

/// Parse every argument the caps depend on exactly once into its
/// canonical form (integers, numbers, `YYYY-MM-DD` dates, validated
/// intervals), refusing what does not parse or is out of range. The cap
/// check and the work both read only this result.
pub fn normalize(a: &Map<String, Value>) -> Result<Map<String, Value>, Value> {
    let mut out = a.clone();
    for (k, max) in INT_ARGS {
        match a.get(*k) {
            None | Some(Value::Null) => {}
            Some(v) => match strict_int(v) {
                Some(n) if (-*max..=*max).contains(&n) => {
                    out.insert((*k).into(), json!(n));
                }
                Some(_) => {
                    return Err(limit_error(format!("'{}' can be at most {}.", k, max)));
                }
                None => {
                    return Err(limit_error(format!("'{}' must be a whole number.", k)));
                }
            },
        }
    }
    for (k, max) in NUM_ARGS {
        match a.get(*k) {
            None | Some(Value::Null) => {}
            Some(v) => match strict_num(v) {
                Some(x) if x.abs() <= *max => {
                    out.insert((*k).into(), json!(x));
                }
                _ => return Err(limit_error(format!("'{}' must be a number.", k))),
            },
        }
    }
    for k in ["start_date", "end_date"] {
        match a.get(k) {
            None | Some(Value::Null) => {}
            // As the web: an empty date is no date.
            Some(Value::String(t)) if t.is_empty() => {
                out.remove(k);
            }
            Some(Value::String(t)) => match strict_date(t) {
                Some(d) => {
                    out.insert(k.into(), json!(d.format("%Y-%m-%d").to_string()));
                }
                None => {
                    return Err(limit_error(format!(
                        "'{}' must be a date written as YYYY-MM-DD between 1900 and 2100.",
                        k
                    )))
                }
            },
            Some(_) => {
                return Err(limit_error(format!(
                    "'{}' must be a date written as YYYY-MM-DD.",
                    k
                )))
            }
        }
    }
    let bad_interval = |raw: &str| {
        limit_error(format!(
            "Interval '{}' is not one The Bridge supports. Use for example 1m, 5m, 15m, 1h, D, W or M.",
            raw
        ))
    };
    if let Some(v) = a.get("interval").filter(|v| !v.is_null()) {
        let raw = v.as_str().unwrap_or_default();
        if Interval::parse(raw).is_none() {
            return Err(bad_interval(raw));
        }
    }
    if let Some(list) = a.get("intervals").and_then(Value::as_array) {
        for v in list {
            let raw = v.as_str().unwrap_or_default();
            if Interval::parse(raw).is_none() {
                return Err(bad_interval(raw));
            }
        }
    }
    Ok(out)
}

const HLC_INDICATORS: &[&str] = &[
    "atr",
    "natr",
    "true_range",
    "adx",
    "adxr",
    "dmi",
    "dx",
    "supertrend",
    "stochastic",
    "stochf",
    "cci",
    "williams_r",
    "keltner",
    "donchian",
    "aroon",
    "aroon_oscillator",
    "psar",
    "ichimoku",
    "pivot_points",
    "ultimate_oscillator",
    "uo_oscillator",
    "chandelier_exit",
    "starc",
    "elderray",
    "ckstop",
    "fractals",
    "rwi",
    "alligator",
    "gator_oscillator",
    "bop",
    "rvi",
    "fisher",
    "avgprice",
    "medprice",
    "midprice",
    "typprice",
    "wclprice",
];
const HLCV_INDICATORS: &[&str] = &["mfi", "cmf", "adl", "emv", "klingervolumeoscillator"];

/// Web `_resolve_inputs`.
pub(super) fn resolve_inputs(
    df: &Frame,
    name: &str,
    inputs: Option<&Vec<Value>>,
) -> Result<(Vec<String>, Vec<Vec<f64>>), PyErr> {
    let cols: Vec<String> = match inputs.filter(|v| !v.is_empty()) {
        Some(v) => v
            .iter()
            .map(|c| c.as_str().unwrap_or_default().to_lowercase())
            .collect(),
        None if HLCV_INDICATORS.contains(&name) => {
            vec!["high".into(), "low".into(), "close".into(), "volume".into()]
        }
        None if HLC_INDICATORS.contains(&name) => vec!["high".into(), "low".into(), "close".into()],
        None => vec!["close".into()],
    };
    let series = cols
        .iter()
        .map(|c| df.col(c))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((cols, series))
}

/// The web's answer for an indicator the desktop does not compute yet.
fn not_available(name: &str) -> Value {
    error(
        format!(
            "The '{}' indicator is not available in The Bridge yet. Indicators available here: {}.",
            name,
            ta::supported().join(", ")
        ),
        &[("error_type", json!("not_available"))],
    )
}

/// Name check shared by calculate_indicator and multi_timeframe_analysis.
pub(super) fn check_indicator(indicator: &str) -> Result<String, Value> {
    let name = indicator.to_lowercase();
    if !ta::WEB_TA_FUNCTIONS.contains(&name.as_str()) {
        return Err(error(format!("unknown indicator '{}'", indicator), &[]));
    }
    if !ta::is_supported(&name) {
        return Err(not_available(&name));
    }
    Ok(name)
}

/// `_last` of a result: one value, or a list for a tuple result.
pub(super) fn latest(out: &Out) -> Value {
    match out {
        Out::One(v) => opt(ta::last(v)),
        Out::Many(v) => Value::Array(v.iter().map(|s| opt(ta::last(s))).collect()),
    }
}

/// Web `_bundle`: latest values, an error object per failing item.
pub(super) fn bundle(items: Vec<(&str, Result<Out, PyErr>)>) -> Value {
    let mut m = Map::new();
    for (k, r) in items {
        let v = match r {
            Ok(o) => latest(&o),
            Err(e) => json!({"error": e.message}),
        };
        m.insert(k.into(), v);
    }
    Value::Object(m)
}

pub(super) fn no_params() -> Map<String, Value> {
    Map::new()
}

pub(super) fn kw(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

pub(super) type ToolResult = Result<Value, (String, PyErr)>;

pub(super) fn tag(action: &str) -> impl Fn(PyErr) -> (String, PyErr) + '_ {
    move |e| (action.to_string(), e)
}

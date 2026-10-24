//! History for the research tools: `/api/v1/history` shaped as the SDK's
//! DataFrame, the interval and window rules, and the records the tools return.

use super::args::{i, s};
use super::limits::{MAX_BARS, MAX_DAYS, MAX_FETCH_BARS};
use crate::mcp::dispatch;
use crate::mcp::envelope::py_repr;
use crate::mcp::ta::{self, PyErr};
use crate::state::AppState;
use chrono::{Datelike, NaiveDate, TimeZone, Utc};
use chrono_tz::Asia::Kolkata;
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// Rows of history indexed by timestamp, as the SDK's `history()` frame.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Epoch seconds, ascending, unique.
    pub ts: Vec<i64>,
    /// Intraday frames carry IST-aware timestamps; daily ones are naive.
    pub intraday: bool,
    /// Every other column, in the reply's key order.
    pub cols: Vec<(String, Vec<Value>)>,
}

impl Frame {
    pub fn len(&self) -> usize {
        self.ts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ts.is_empty()
    }

    /// A column as floats (`df[name]`), or the KeyError pandas raises.
    pub fn col(&self, name: &str) -> Result<Vec<f64>, PyErr> {
        self.cols
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.iter().map(|x| x.as_f64().unwrap_or(f64::NAN)).collect())
            .ok_or_else(|| PyErr {
                kind: "KeyError",
                message: format!("'{}'", name),
            })
    }

    pub(super) fn close(&self) -> Vec<f64> {
        self.col("close").unwrap_or_default()
    }

    /// The last `n` rows (`df.tail(n)`).
    pub fn tail(&self, n: usize) -> Frame {
        let start = self.len().saturating_sub(n);
        Frame {
            ts: self.ts[start..].to_vec(),
            intraday: self.intraday,
            cols: self
                .cols
                .iter()
                .map(|(k, v)| (k.clone(), v[start..].to_vec()))
                .collect(),
        }
    }

    /// `pandas.Timestamp.isoformat()` of row `i` (web `_idx_iso`).
    pub fn iso(&self, i: usize) -> String {
        iso(self.ts[i], self.intraday)
    }
}

fn iso(ts: i64, intraday: bool) -> String {
    let utc = Utc.timestamp_opt(ts, 0).single().unwrap_or_default();
    if intraday {
        utc.with_timezone(&Kolkata)
            .format("%Y-%m-%dT%H:%M:%S%:z")
            .to_string()
    } else {
        utc.naive_utc().format("%Y-%m-%dT%H:%M:%S").to_string()
    }
}

/// `DataFrame.to_json(date_format="iso")` timestamp: UTC with
/// milliseconds, `Z` when the index is timezone-aware.
fn json_iso(ts: i64, intraday: bool) -> String {
    let utc = Utc.timestamp_opt(ts, 0).single().unwrap_or_default();
    let s = utc.naive_utc().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
    if intraday {
        format!("{}Z", s)
    } else {
        s
    }
}

/// A float as `to_json` writes it (10 decimal places, NaN as null).
fn json_float(x: f64) -> Value {
    if x.is_finite() {
        json!(ta::py_round(x, 10))
    } else {
        Value::Null
    }
}

/// Python slice `seq[-n:]` start index for a list of `len`.
pub(super) fn tail_start(len: usize, n: i64) -> usize {
    if n == 0 {
        0
    } else if n > 0 {
        len.saturating_sub(n as usize)
    } else {
        (n.unsigned_abs() as usize).min(len)
    }
}

/// Web `_df_records(df, limit)`: the last `limit` rows (all when 0) as
/// records with a `timestamp` key.
pub fn records(
    ts: &[i64],
    intraday: bool,
    cols: &[(String, Vec<Value>)],
    limit: i64,
) -> Vec<Value> {
    let start = if limit == 0 {
        0
    } else {
        tail_start(ts.len(), limit)
    };
    (start..ts.len())
        .map(|i| {
            let mut m = Map::new();
            m.insert("timestamp".into(), json!(json_iso(ts[i], intraday)));
            for (k, v) in cols {
                let cell = match &v[i] {
                    Value::Number(n) if n.is_f64() => json_float(n.as_f64().unwrap_or(f64::NAN)),
                    other => other.clone(),
                };
                m.insert(k.clone(), cell);
            }
            Value::Object(m)
        })
        .collect()
}

pub(super) fn f64_col(v: &[f64]) -> Vec<Value> {
    v.iter().map(|x| json!(*x)).collect()
}

pub(super) fn opt(x: Option<f64>) -> Value {
    x.map(|v| json!(v)).unwrap_or(Value::Null)
}

/// Build the frame from the SDK reply's `data` (sorted, duplicates dropped).
pub(super) fn frame_from(data: &[Value], intraday: bool) -> Frame {
    let mut rows: Vec<(i64, &Map<String, Value>)> = data
        .iter()
        .filter_map(|r| {
            let o = r.as_object()?;
            let t = o.get("timestamp")?;
            let ts = t.as_i64().or_else(|| t.as_f64().map(|f| f as i64))?;
            Some((ts, o))
        })
        .collect();
    rows.sort_by_key(|(t, _)| *t);
    rows.dedup_by_key(|(t, _)| *t);
    let mut names: Vec<String> = Vec::new();
    if let Some((_, first)) = rows.first() {
        for k in first.keys() {
            if k != "timestamp" {
                names.push(k.clone());
            }
        }
    }
    let cols = names
        .into_iter()
        .map(|n| {
            let vals: Vec<Value> = rows
                .iter()
                .map(|(_, o)| o.get(&n).cloned().unwrap_or(Value::Null))
                .collect();
            // A column holding any float is a float column in pandas.
            let float = vals
                .iter()
                .any(|v| v.as_number().is_some_and(|x| x.is_f64()));
            let vals = if float {
                vals.into_iter()
                    .map(|v| v.as_f64().map(|f| json!(f)).unwrap_or(Value::Null))
                    .collect()
            } else {
                vals
            };
            (n, vals)
        })
        .collect();
    Frame {
        ts: rows.iter().map(|(t, _)| *t).collect(),
        intraday,
        cols,
    }
}

/// Web `_history_df`: the SDK's `history()` then the two refusals.
async fn history_df(
    ctx: &Arc<AppState>,
    symbol: &str,
    exchange: &str,
    itv: &Interval,
    start: &str,
    end: &str,
    source: &str,
) -> Result<Frame, PyErr> {
    let mut p = Map::new();
    p.insert("symbol".into(), json!(symbol.to_uppercase()));
    p.insert("exchange".into(), json!(exchange.to_uppercase()));
    p.insert("interval".into(), json!(itv.raw));
    p.insert("start_date".into(), json!(start));
    p.insert("end_date".into(), json!(end));
    p.insert("source".into(), json!(source));
    let result = dispatch::sdk_post(ctx, "history", p).await;
    let ok = result.get("status").and_then(Value::as_str) == Some("success");
    let reply = match (ok, result.get("data").and_then(Value::as_array)) {
        (true, Some(rows)) if rows.is_empty() => json!({
            "status": "error",
            "message": "No data available for the specified period",
            "error_type": "no_data",
        }),
        (true, Some(rows)) => {
            let f = frame_from(rows, itv.intraday());
            if f.is_empty() {
                return Err(PyErr::value(
                    "no historical data returned for the given range",
                ));
            }
            return Ok(f);
        }
        _ => result,
    };
    Err(PyErr::value(format!("history error: {}", py_repr(&reply))))
}

/// A history interval, parsed once with a strict grammar: an optional
/// multiplier (1 to 999, digits only) and a unit, `s`, `m` or `h` (which
/// need the multiplier) or `D`, `W`, `M`, `Q`, `Y`. No spaces, signs or
/// other spellings: what passes is exactly what is sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Interval {
    pub raw: String,
    mult: u32,
    unit: char,
}

impl Interval {
    pub fn parse(s: &str) -> Option<Interval> {
        let unit = s.chars().last()?;
        let digits = &s[..s.len() - unit.len_utf8()];
        if !matches!(unit, 's' | 'm' | 'h' | 'D' | 'W' | 'M' | 'Q' | 'Y') {
            return None;
        }
        let mult = if digits.is_empty() {
            if matches!(unit, 's' | 'm' | 'h') {
                return None;
            }
            1
        } else {
            if digits.len() > 3 || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            digits.parse::<u32>().ok().filter(|m| *m >= 1)?
        };
        Some(Interval {
            raw: s.to_string(),
            mult,
            unit,
        })
    }

    /// Web `_BARS_PER_DAY` (used, as on the web, only to size the calendar
    /// window for a bar count).
    fn web_bars_per_day(&self) -> f64 {
        match self.raw.to_lowercase().as_str() {
            "1m" => 375.0,
            "3m" => 125.0,
            "5m" => 75.0,
            "10m" => 38.0,
            "15m" => 25.0,
            "30m" => 13.0,
            "1h" | "60m" => 7.0,
            "2h" => 4.0,
            "3h" => 3.0,
            "4h" => 2.0,
            "d" | "day" => 1.0,
            "w" | "week" => 0.2,
            "m" | "month" => 0.05,
            _ => 75.0,
        }
    }

    /// Bars a trading day (375 minutes) really holds at this interval,
    /// rounded up: what the caps count.
    pub fn true_bars_per_day(&self) -> f64 {
        let k = f64::from(self.mult);
        match self.unit {
            's' => (22_500.0 / k).ceil(),
            'm' => (375.0 / k).ceil(),
            'h' => (6.25 / k).ceil(),
            'D' => 1.0 / k,
            'W' => 0.2 / k,
            'M' => 0.05 / k,
            'Q' => 1.0 / 63.0 / k,
            _ => 1.0 / 252.0 / k,
        }
    }

    fn intraday(&self) -> bool {
        !matches!(self.raw.as_str(), "D" | "W" | "M")
    }
}

/// A date argument: exactly `YYYY-MM-DD`, years 1900 to 2100.
pub fn strict_date(s: &str) -> Option<NaiveDate> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    if !b
        .iter()
        .enumerate()
        .all(|(k, c)| k == 4 || k == 7 || c.is_ascii_digit())
    {
        return None;
    }
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    (1900..=2100).contains(&d.year()).then_some(d)
}

/// The fetch window of one history call, resolved once from the
/// canonical arguments: the cap check and the fetch use this same value.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub start: NaiveDate,
    pub end: NaiveDate,
    /// Keep only the last N bars (`lookback_bars` windows).
    pub tail: Option<usize>,
    pub source: String,
}

/// Web `_load_history`'s window: explicit range, else the last N days,
/// else enough calendar days for `lookback_bars` bars. All arithmetic is
/// checked; a window past the caps is refused with a trader-facing message.
pub fn resolve(
    a: &Map<String, Value>,
    itv: &Interval,
    lookback_bars: i64,
    today: NaiveDate,
) -> Result<Window, String> {
    let date = |k: &str| a.get(k).and_then(Value::as_str).and_then(strict_date);
    let end = date("end_date").unwrap_or(today);
    let shift = |days: i64| -> Option<NaiveDate> {
        let n = chrono::Days::new(days.unsigned_abs());
        if days >= 0 {
            end.checked_sub_days(n)
        } else {
            end.checked_add_days(n)
        }
    };
    let too_far = || {
        "That date range is outside what The Bridge can analyse. Use dates between 1900 and 2100."
            .to_string()
    };
    let (start, tail) = if let Some(s) = date("start_date") {
        (s, None)
    } else if let Some(days) = i(a, "lookback_days").filter(|d| *d != 0) {
        (shift(days).ok_or_else(too_far)?, None)
    } else {
        let bars = lookback_bars.clamp(-MAX_BARS, MAX_BARS);
        let cal_days = ((bars as f64 / itv.web_bars_per_day()) * 1.6) as i64 + 5;
        let tail = usize::try_from(bars).ok();
        (shift(cal_days).ok_or_else(too_far)?, tail)
    };
    let span = (end - start).num_days().unsigned_abs();
    if span > MAX_DAYS as u64 {
        return Err(format!(
            "The date range can span at most {} days. Narrow start_date and end_date.",
            MAX_DAYS
        ));
    }
    if (span as f64 + 1.0) * itv.true_bars_per_day() > MAX_FETCH_BARS {
        return Err(format!(
            "That much history at interval '{}' is too large to analyse in one call \
(at most about {} bars). Use a shorter range or a longer interval.",
            itv.raw, MAX_FETCH_BARS as i64
        ));
    }
    Ok(Window {
        start,
        end,
        tail,
        source: s(a, "source").unwrap_or_else(|| "api".into()),
    })
}

/// Fetch one window (the SDK's `history()`), trimmed to its tail.
pub async fn load_history(
    ctx: &Arc<AppState>,
    symbol: &str,
    exchange: &str,
    itv: &Interval,
    w: &Window,
) -> Result<Frame, PyErr> {
    let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
    let f = history_df(
        ctx,
        symbol,
        exchange,
        itv,
        &fmt(w.start),
        &fmt(w.end),
        &w.source,
    )
    .await?;
    Ok(match w.tail {
        Some(n) => f.tail(n),
        None => f,
    })
}

/// Interval and window of a one-interval tool, from the canonical
/// arguments (the same computation `admit` checked).
pub(super) fn plan(
    a: &Map<String, Value>,
    default_interval: &str,
    lookback_bars: i64,
    today: NaiveDate,
) -> Result<(Interval, Window), PyErr> {
    let raw = s(a, "interval").unwrap_or_else(|| default_interval.to_string());
    let itv = Interval::parse(&raw)
        .ok_or_else(|| PyErr::value(format!("unsupported interval '{}'", raw)))?;
    let w = resolve(a, &itv, lookback_bars, today).map_err(PyErr::value)?;
    Ok((itv, w))
}

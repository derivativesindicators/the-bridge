//! The Bridge 1.0.8: optional behaviours of a signal-driven strategy, each
//! off by default so plain TradingView BUY/SELL automation is unchanged.
//!
//! * **Overnight carry** (`carry_overnight`): open positions are not closed
//!   at the first alert of a new day; they stay until an exit or flip
//!   signal, a stop/target/trail, or an expiry roll.
//! * **Expiry-day roll** (`expiry_roll`): on the held contract's expiry day
//!   at `roll_time`, close each open leg expiring today and reopen it in
//!   the next expiry, re-resolved by the leg's own strike rule (the
//!   backtest engine's `reopen_strike: recompute`), at the next session's
//!   `reopen_time` (the engine's rule) or immediately.
//! * **Entry deferral** (`min_days_to_expiry`, `expiry_day_entry`): an
//!   entry into a contract too close to expiry waits for the next session's
//!   `reopen_time` (the engine's `defer`).
//! * **CAS exit** (`cas_exit`): from `cas_start` to `session_end`, track a
//!   Supertrend on the traded option's premium and close the position when
//!   it flips against it (the engine's `cas_mode: cas_close`).

use chrono::NaiveTime;
use serde_json::{json, Map, Value};

pub const CAS_TIMEFRAMES: &[i64] = &[1, 3, 5, 10, 15, 30, 60];
pub const REOPEN_MODES: &[&str] = &["next_session", "immediate"];
pub const EXPIRY_DAY_ENTRY: &[&str] = &["same", "defer"];
pub const CAS_LEGS: &[&str] = &["auto", "sell", "buy"];
pub const CAS_AT_END: &[&str] = &["hold", "close"];
pub const SESSION_TIMES: &[&str] = &["auto", "custom"];

/// SEBI's October 2026 review takes effect: cash CAS moves to 15:30-15:40,
/// derivatives trade to 15:45, expiring index contracts stop at 15:30.
pub fn regime_change() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 12, 7).unwrap_or_default()
}
/// The latest an expiring contract may be rolled (both regimes).
pub fn expiring_contract_close() -> NaiveTime {
    t(15, 30)
}

const FIELDS: &[&str] = &[
    "carry_overnight",
    "expiry_roll",
    "roll_time",
    "reopen",
    "reopen_time",
    "min_days_to_expiry",
    "expiry_day_entry",
    "cas_exit",
    "cas_start",
    "session_end",
    "cas_timeframe",
    "cas_st_atr",
    "cas_st_mult",
    "cas_leg",
    "cas_at_end",
    "session_times",
];

/// Parsed options with every default filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalOptions {
    pub carry_overnight: bool,
    pub expiry_roll: bool,
    pub roll_time: NaiveTime,
    pub reopen: String,
    pub reopen_time: NaiveTime,
    pub min_days_to_expiry: i64,
    pub expiry_day_entry: String,
    pub cas_exit: bool,
    pub cas_start: NaiveTime,
    pub session_end: NaiveTime,
    pub cas_timeframe: i64,
    pub cas_st_atr: usize,
    pub cas_st_mult: f64,
    pub cas_leg: String,
    pub cas_at_end: String,
    /// `auto`: the exchange timetable by date (see `effective`); `custom`:
    /// `cas_start` / `session_end` as entered.
    pub session_times: String,
}

fn t(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).unwrap_or_default()
}

impl Default for SignalOptions {
    fn default() -> Self {
        Self {
            carry_overnight: false,
            expiry_roll: false,
            roll_time: t(15, 25),
            reopen: "next_session".into(),
            reopen_time: t(9, 20),
            min_days_to_expiry: 0,
            expiry_day_entry: "same".into(),
            cas_exit: false,
            cas_start: t(15, 15),
            session_end: t(15, 40),
            cas_timeframe: 5,
            cas_st_atr: 5,
            cas_st_mult: 2.0,
            cas_leg: "auto".into(),
            cas_at_end: "hold".into(),
            session_times: "auto".into(),
        }
    }
}

impl SignalOptions {
    /// From a stored (already validated) value; anything unreadable is the
    /// default, so a bad row can only ever mean "off".
    pub fn from_value(v: &Value) -> Self {
        let d = Self::default();
        let b = |k: &str| v[k].as_bool().unwrap_or(false);
        let tm = |k: &str, def: NaiveTime| {
            v[k].as_str()
                .and_then(|s| NaiveTime::parse_from_str(s.trim(), "%H:%M").ok())
                .unwrap_or(def)
        };
        let s = |k: &str, def: &str| v[k].as_str().unwrap_or(def).to_string();
        Self {
            carry_overnight: b("carry_overnight"),
            expiry_roll: b("expiry_roll"),
            roll_time: tm("roll_time", d.roll_time),
            reopen: s("reopen", &d.reopen),
            reopen_time: tm("reopen_time", d.reopen_time),
            min_days_to_expiry: v["min_days_to_expiry"].as_i64().unwrap_or(0),
            expiry_day_entry: s("expiry_day_entry", &d.expiry_day_entry),
            cas_exit: b("cas_exit"),
            cas_start: tm("cas_start", d.cas_start),
            session_end: tm("session_end", d.session_end),
            cas_timeframe: v["cas_timeframe"].as_i64().unwrap_or(d.cas_timeframe),
            cas_st_atr: v["cas_st_atr"].as_u64().map(|n| n as usize).unwrap_or(d.cas_st_atr),
            cas_st_mult: v["cas_st_mult"].as_f64().unwrap_or(d.cas_st_mult),
            cas_leg: s("cas_leg", &d.cas_leg),
            cas_at_end: s("cas_at_end", &d.cas_at_end),
            session_times: s("session_times", &d.session_times),
        }
    }

    /// The CAS window `(start, F&O session end)` on `date`. With `auto`:
    /// before 7 Dec 2026, 15:15 (spot's last 1m bar 15:14) to 15:40; from
    /// then, 15:30 to 15:45. On a held contract's expiry day the window ends
    /// no later than 15:30 from 7 Dec 2026 (expiring index contracts stop
    /// then).
    pub fn effective(&self, date: chrono::NaiveDate, expiring_today: bool) -> (NaiveTime, NaiveTime) {
        let (start, end) = if self.session_times == "custom" {
            (self.cas_start, self.session_end)
        } else if date >= regime_change() {
            (t(15, 30), t(15, 45))
        } else {
            (t(15, 15), t(15, 40))
        };
        if expiring_today && date >= regime_change() {
            (start, end.min(expiring_contract_close()))
        } else {
            (start, end)
        }
    }

    /// Whether entries may be deferred (either deferral rule is set).
    pub fn defers(&self) -> bool {
        self.min_days_to_expiry > 0 || self.expiry_day_entry == "defer"
    }

    /// Whether anything here needs the per-second timers.
    pub fn any_timer(&self) -> bool {
        self.expiry_roll || self.cas_exit
    }
}

fn hhmm(m: &Map<String, Value>, k: &str, def: &str) -> Result<String, String> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(def.to_string()),
        Some(Value::String(s)) => NaiveTime::parse_from_str(s.trim(), "%H:%M")
            .map(|t| t.format("%H:%M").to_string())
            .map_err(|_| format!("signal_options.{} must be a time as HH:MM", k)),
        _ => Err(format!("signal_options.{} must be a time as HH:MM", k)),
    }
}

fn flag(m: &Map<String, Value>, k: &str) -> Result<bool, String> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(format!("signal_options.{} must be true or false", k)),
    }
}

fn pick(m: &Map<String, Value>, k: &str, allowed: &[&str]) -> Result<String, String> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(allowed[0].to_string()),
        Some(Value::String(s)) => {
            let s = s.trim().to_ascii_lowercase();
            if allowed.contains(&s.as_str()) {
                Ok(s)
            } else {
                Err(format!("signal_options.{} must be one of {}", k, allowed.join(", ")))
            }
        }
        _ => Err(format!("signal_options.{} must be one of {}", k, allowed.join(", "))),
    }
}

fn int(m: &Map<String, Value>, k: &str, def: i64, lo: i64, hi: i64) -> Result<i64, String> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(def),
        Some(v) => match v.as_f64() {
            Some(f) if f.fract() == 0.0 && f >= lo as f64 && f <= hi as f64 => Ok(f as i64),
            _ => Err(format!(
                "signal_options.{} must be a whole number from {} to {}",
                k, lo, hi
            )),
        },
    }
}

/// Validate `signal_options` against the strategy it belongs to. `None` or
/// `null` stays `null` (everything off).
pub fn validate(
    v: Option<&Value>,
    kind: &str,
    strategy_type: &str,
    product: &str,
) -> Result<Value, String> {
    let m = match v {
        None | Some(Value::Null) => return Ok(Value::Null),
        Some(Value::Object(m)) => m,
        Some(_) => return Err("signal_options must be an object".into()),
    };
    if kind != "signal" {
        return Err("signal_options apply to signal-driven strategies only".into());
    }
    for k in m.keys() {
        if !FIELDS.contains(&k.as_str()) {
            return Err(format!("signal_options.{} is not a known option", k));
        }
    }
    let carry = flag(m, "carry_overnight")?;
    let roll = flag(m, "expiry_roll")?;
    let cas = flag(m, "cas_exit")?;
    let reopen = pick(m, "reopen", REOPEN_MODES)?;
    let expiry_day_entry = pick(m, "expiry_day_entry", EXPIRY_DAY_ENTRY)?;
    let min_days = int(m, "min_days_to_expiry", 0, 0, 30)?;
    let tf = int(m, "cas_timeframe", 5, 1, 60)?;
    if !CAS_TIMEFRAMES.contains(&tf) {
        return Err(format!(
            "signal_options.cas_timeframe must be one of {:?} minutes",
            CAS_TIMEFRAMES
        ));
    }
    let atr = int(m, "cas_st_atr", 5, 1, 100)?;
    let mult = match m.get("cas_st_mult") {
        None | Some(Value::Null) => 2.0,
        Some(v) => match v.as_f64() {
            Some(f) if f.is_finite() && (0.1..=20.0).contains(&f) => f,
            _ => return Err("signal_options.cas_st_mult must be a number from 0.1 to 20".into()),
        },
    };
    let out = json!({
        "carry_overnight": carry,
        "expiry_roll": roll,
        "roll_time": hhmm(m, "roll_time", "15:25")?,
        "reopen": reopen,
        "reopen_time": hhmm(m, "reopen_time", "09:20")?,
        "min_days_to_expiry": min_days,
        "expiry_day_entry": expiry_day_entry,
        "cas_exit": cas,
        "cas_start": hhmm(m, "cas_start", "15:15")?,
        "session_end": hhmm(m, "session_end", "15:40")?,
        "cas_timeframe": tf,
        "cas_st_atr": atr,
        "cas_st_mult": mult,
        "cas_leg": pick(m, "cas_leg", CAS_LEGS)?,
        "cas_at_end": pick(m, "cas_at_end", CAS_AT_END)?,
        "session_times": pick(m, "session_times", SESSION_TIMES)?,
    });
    let o = SignalOptions::from_value(&out);
    let carries = carry || (roll && o.reopen == "next_session");
    if carries {
        if strategy_type != "positional" {
            return Err(if carry {
                "Overnight carry needs a positional strategy (an intraday one is squared off at its exit time).".into()
            } else {
                "An expiry roll that reopens next session holds positions overnight: turn on overnight carry (positional, NRML), or reopen immediately.".into()
            });
        }
        if !carry {
            return Err("An expiry roll that reopens next session needs overnight carry on, or set it to reopen immediately.".into());
        }
        if !product.eq_ignore_ascii_case("NRML") {
            return Err("Overnight carry needs product NRML: the broker squares MIS positions off before the close.".into());
        }
    }
    if cas && o.cas_start >= o.session_end {
        return Err("signal_options.cas_start must be earlier than session_end".into());
    }
    if roll && o.roll_time >= expiring_contract_close() {
        return Err("signal_options.roll_time must be before 15:30: expiring index contracts stop trading at 15:30 on expiry day from 7 Dec 2026".into());
    }
    Ok(out)
}

/// Pine `ta.supertrend`-style state, as the backtest engine computes it
/// (`strategy.supertrend`: Wilder ATR seeded with the mean of the first
/// `periods` true ranges, hl2 bands that only tighten, trend starting up).
#[derive(Debug, Clone)]
pub struct Supertrend {
    periods: usize,
    mult: f64,
    trs: Vec<f64>,
    atr: Option<f64>,
    prev_close: Option<f64>,
    up: Option<f64>,
    dn: Option<f64>,
    pub trend: i8,
    pub candles: usize,
}

impl Supertrend {
    pub fn new(periods: usize, mult: f64) -> Self {
        Self {
            periods: periods.max(1),
            mult,
            trs: Vec::new(),
            atr: None,
            prev_close: None,
            up: None,
            dn: None,
            trend: 1,
            candles: 0,
        }
    }

    /// Whether the ATR has its first value (enough candles seen).
    pub fn ready(&self) -> bool {
        self.atr.is_some()
    }

    /// Add one closed candle; returns the trend after it.
    pub fn push(&mut self, h: f64, l: f64, c: f64) -> i8 {
        self.candles += 1;
        let tr = match self.prev_close {
            None => h - l,
            Some(pc) => (h - l).max((h - pc).abs()).max((l - pc).abs()),
        };
        self.atr = match self.atr {
            Some(a) => Some((a * (self.periods as f64 - 1.0) + tr) / self.periods as f64),
            None => {
                self.trs.push(tr);
                if self.trs.len() == self.periods {
                    Some(self.trs.iter().sum::<f64>() / self.periods as f64)
                } else {
                    None
                }
            }
        };
        if let Some(atr) = self.atr {
            let hl2 = (h + l) / 2.0;
            let mut u = hl2 - self.mult * atr;
            let mut d = hl2 + self.mult * atr;
            let u1 = self.up.unwrap_or(u);
            let d1 = self.dn.unwrap_or(d);
            if let Some(pc) = self.prev_close {
                if pc > u1 {
                    u = u.max(u1);
                }
                if pc < d1 {
                    d = d.min(d1);
                }
            }
            self.trend = if self.trend == -1 && c > d1 {
                1
            } else if self.trend == 1 && c < u1 {
                -1
            } else {
                self.trend
            };
            self.up = Some(u);
            self.dn = Some(d);
        }
        self.prev_close = Some(c);
        self.trend
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_is_everything_off() {
        assert_eq!(validate(None, "signal", "intraday", "MIS").unwrap(), Value::Null);
        let o = SignalOptions::from_value(&Value::Null);
        assert!(!o.carry_overnight && !o.expiry_roll && !o.cas_exit && !o.defers());
        assert_eq!(o.session_end, t(15, 40));
        let d = |y, m, dd| chrono::NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        assert_eq!(o.effective(d(2026, 11, 3), false), (t(15, 15), t(15, 40)));
        assert_eq!(o.effective(d(2026, 11, 3), true), (t(15, 15), t(15, 40)));
        assert_eq!(o.effective(d(2026, 12, 7), false), (t(15, 30), t(15, 45)));
        assert_eq!(o.effective(d(2026, 12, 8), true), (t(15, 30), t(15, 30)));
        let c = SignalOptions::from_value(&json!({"session_times": "custom", "cas_start": "15:20", "session_end": "15:39"}));
        assert_eq!(c.effective(d(2026, 11, 3), false), (t(15, 20), t(15, 39)));
    }

    #[test]
    fn carry_needs_positional_nrml() {
        let on = json!({"carry_overnight": true});
        assert!(validate(Some(&on), "signal", "intraday", "NRML").is_err());
        assert!(validate(Some(&on), "signal", "positional", "MIS").unwrap_err().contains("NRML"));
        assert!(validate(Some(&on), "signal", "positional", "NRML").is_ok());
        assert!(validate(Some(&on), "batch", "positional", "NRML").is_err());
    }

    #[test]
    fn next_session_roll_needs_carry_immediate_does_not() {
        let r = json!({"expiry_roll": true});
        assert!(validate(Some(&r), "signal", "positional", "NRML").is_err());
        let r = json!({"expiry_roll": true, "reopen": "immediate"});
        assert!(validate(Some(&r), "signal", "intraday", "MIS").is_ok());
        let r = json!({"expiry_roll": true, "carry_overnight": true, "roll_time": "15:30"});
        assert!(validate(Some(&r), "signal", "positional", "NRML").is_err());
    }

    #[test]
    fn fields_are_checked() {
        for bad in [
            json!({"x": 1}),
            json!({"cas_exit": "yes"}),
            json!({"cas_timeframe": 7}),
            json!({"cas_st_mult": 0}),
            json!({"cas_start": "3pm"}),
            json!({"cas_exit": true, "cas_start": "15:30", "session_end": "15:30"}),
            json!({"cas_leg": "both"}),
            json!({"min_days_to_expiry": -1}),
        ] {
            assert!(validate(Some(&bad), "signal", "intraday", "MIS").is_err(), "{}", bad);
        }
        let ok = validate(
            Some(&json!({"cas_exit": true, "session_end": "15:39", "cas_timeframe": 3, "cas_st_atr": 10, "cas_st_mult": 3})),
            "signal",
            "intraday",
            "MIS",
        )
        .unwrap();
        assert_eq!(ok["session_end"], "15:39");
        assert_eq!(ok["cas_st_mult"], 3.0);
    }

    #[test]
    fn supertrend_flips_down_on_a_fall_and_up_on_a_rise() {
        let mut st = Supertrend::new(5, 2.0);
        for i in 0..20 {
            let c = 100.0 + i as f64;
            st.push(c + 1.0, c - 1.0, c);
        }
        assert!(st.ready());
        assert_eq!(st.trend, 1);
        let mut flipped_down = false;
        for i in 0..15 {
            let c = 118.0 - 4.0 * i as f64;
            if st.push(c + 1.0, c - 1.0, c) == -1 {
                flipped_down = true;
                break;
            }
        }
        assert!(flipped_down);
        let mut flipped_up = false;
        for i in 0..15 {
            let c = 70.0 + 5.0 * i as f64;
            if st.push(c + 1.0, c - 1.0, c) == 1 {
                flipped_up = true;
                break;
            }
        }
        assert!(flipped_up);
    }
}

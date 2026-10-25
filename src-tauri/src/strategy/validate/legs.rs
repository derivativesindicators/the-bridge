//! Validating a strategy's legs (web `blueprints/strategy_module.py` leg
//! rules): a batch leg's contract and risk fields, a signal leg's symbol
//! and quantity, and the list as a whole.

use super::{
    choice, get, integer, mapping, number, py_repr, reject_unknown, required, risk_fields, text,
    LEG_EXPIRIES, LEG_FIELDS, LEG_OPTION_TYPES, LEG_POSITIONS, LEG_SEGMENTS, LEG_SIDES,
    LEG_STRIKE_MODES, MAX_CASH_QUANTITY, MAX_LEGS, MAX_LOTS, MAX_SIGNAL_LOTS, MAX_SIGNAL_QTY,
    MIN_LEGS, QTY_MODES, R, SIGNAL_LEG_EXCHANGES, SIGNAL_LEG_FIELDS,
};
use crate::brokers::common::symbols::SymbolGeneration;
use crate::strategy::resolver::{is_derivative_exchange, quantity_is_whole_lots};
use serde_json::{json, Map, Value};

fn leg_id(leg: &Map<String, Value>, label: &str, index: usize) -> R<i64> {
    match get(leg, "id") {
        Some(v) => integer(v, &format!("{}.id", label), 1, MAX_LEGS as i64),
        None => Ok(index as i64 + 1),
    }
}

/// The Bridge 1.0.6: is this a signal leg whose contract is chosen when the
/// signal arrives (no `symbol`, a batch-style `position`)? It trades the
/// strategy's underlying: a future or an ATM-relative option, resolved
/// with the batch resolver at signal time.
pub fn is_resolved_signal_leg(leg: &Value) -> bool {
    leg.get("symbol").map_or(true, Value::is_null) && leg.get("position").is_some()
}

/// A resolved signal leg: the batch leg's contract and risk fields, plus
/// the signal `side` it fires on (long or short; never both, because the
/// contract and the order side differ per direction).
fn validate_resolved_signal_leg(raw: &Value, index: usize) -> R<Value> {
    let label = format!("legs[{}]", index);
    let leg = mapping(raw, &label)?;
    let side = choice(
        required(leg, "side", &label)?,
        &["long", "short"],
        &format!("{}.side", label),
    )?;
    let mut rest = leg.clone();
    rest.remove("side");
    rest.remove("symbol");
    let mut c = match validate_leg(&Value::Object(rest), index)? {
        Value::Object(m) => m,
        _ => unreachable!(),
    };
    if c["segment"] == "cash" {
        return Err(format!(
            "{}.segment is 'cash'. A leg chosen at signal time trades futures or options; name a cash symbol instead.",
            label
        ));
    }
    c.insert("side".into(), json!(side));
    Ok(Value::Object(c))
}

fn validate_signal_leg(raw: &Value, index: usize, g: &SymbolGeneration) -> R<Value> {
    if is_resolved_signal_leg(raw) {
        return validate_resolved_signal_leg(raw, index);
    }
    let label = format!("legs[{}]", index);
    let leg = mapping(raw, &label)?;
    reject_unknown(leg, SIGNAL_LEG_FIELDS, &label)?;
    let segment = choice(
        get(leg, "segment").unwrap_or(&json!("cash")),
        &["cash", "futures"],
        &format!("{}.segment", label),
    )?;
    let mut c = Map::new();
    c.insert("id".into(), json!(leg_id(leg, &label, index)?));
    let symbol = text(
        required(leg, "symbol", &label)?,
        &format!("{}.symbol", label),
        100,
    )?
    .to_ascii_uppercase();
    let exch_text = text(
        required(leg, "exchange", &label)?,
        &format!("{}.exchange", label),
        20,
    )?
    .to_ascii_uppercase();
    let exchange = choice(
        &json!(exch_text),
        SIGNAL_LEG_EXCHANGES,
        &format!("{}.exchange", label),
    )?;
    c.insert("symbol".into(), json!(symbol));
    c.insert("exchange".into(), json!(exchange));
    c.insert(
        "side".into(),
        json!(choice(
            get(leg, "side").unwrap_or(&json!("both")),
            LEG_SIDES,
            &format!("{}.side", label)
        )?),
    );
    c.insert("segment".into(), json!(segment));
    let derivative = is_derivative_exchange(exchange);
    let qty_mode = choice(
        get(leg, "qty_mode").unwrap_or(&json!(if derivative { "lots" } else { "units" })),
        QTY_MODES,
        &format!("{}.qty_mode", label),
    )?;
    if qty_mode == "lots" && !derivative {
        return Err(format!(
            "{}.qty_mode is 'lots', but {} has no lot size. Cash instruments are counted in units.",
            label, exchange
        ));
    }
    c.insert("qty_mode".into(), json!(qty_mode));
    let qty = integer(
        required(leg, "qty", &label)?,
        &format!("{}.qty", label),
        1,
        if qty_mode == "lots" {
            MAX_SIGNAL_LOTS
        } else {
            MAX_SIGNAL_QTY
        },
    )?;
    c.insert("qty".into(), json!(qty));
    if segment == "futures" {
        c.insert(
            "expiry".into(),
            json!(choice(
                get(leg, "expiry").unwrap_or(&json!("current")),
                LEG_EXPIRIES,
                &format!("{}.expiry", label)
            )?),
        );
    } else if get(leg, "expiry").is_some() {
        return Err(format!("{}.expiry does not apply to a cash leg", label));
    }
    risk_fields(leg, &mut c, &label, &label)?;
    if qty_mode == "units" {
        let (whole, lot) = quantity_is_whole_lots(g, qty, &symbol, exchange);
        if !whole {
            return Err(format!(
                "{}.qty is {}, which is not a whole number of lots. {} on {} trades in lots of {}.",
                label,
                qty,
                symbol,
                exchange,
                lot.unwrap_or(0)
            ));
        }
    }
    Ok(Value::Object(c))
}

/// An options leg's contract fields: the option type, and either the ATM
/// offset or a fixed strike (never both).
fn option_fields(leg: &Map<String, Value>, c: &mut Map<String, Value>, label: &str) -> R<()> {
    c.insert(
        "option_type".into(),
        json!(choice(
            required(leg, "option_type", label)?,
            LEG_OPTION_TYPES,
            &format!("{}.option_type", label)
        )?),
    );
    let mode = choice(
        get(leg, "strike_mode").unwrap_or(&json!("atm")),
        LEG_STRIKE_MODES,
        &format!("{}.strike_mode", label),
    )?;
    c.insert("strike_mode".into(), json!(mode));
    if mode == "atm" {
        if get(leg, "strike").is_some() {
            return Err(format!(
                "{}.strike is only used when strike_mode is 'strike'. Set strike_mode to 'strike', or remove the strike.",
                label
            ));
        }
        let offsets: Vec<String> = std::iter::once("ATM".to_string())
            .chain((1..=5).map(|n| format!("ITM{}", n)))
            .chain((1..=5).map(|n| format!("OTM{}", n)))
            .collect();
        let raw_off = get(leg, "atm_offset").cloned().unwrap_or(json!("ATM"));
        let t = raw_off.as_str().unwrap_or("").trim().to_ascii_uppercase();
        if !offsets.contains(&t) {
            return Err(format!(
                "{}.atm_offset must be one of: {}. Got {}",
                label,
                offsets.join(", "),
                py_repr(&raw_off)
            ));
        }
        c.insert("atm_offset".into(), json!(t));
    } else {
        if get(leg, "atm_offset").is_some() {
            return Err(format!(
                "{}.atm_offset is only used when strike_mode is 'atm'. Set strike_mode to 'atm', or remove the offset.",
                label
            ));
        }
        c.insert(
            "strike".into(),
            number(
                required(leg, "strike", label)?,
                &format!("{}.strike", label),
                None,
                None,
                Some(0.0),
            )?,
        );
    }
    Ok(())
}

fn validate_leg(raw: &Value, index: usize) -> R<Value> {
    let label = format!("legs[{}]", index);
    let leg = mapping(raw, &label)?;
    reject_unknown(leg, LEG_FIELDS, &label)?;
    let segment = choice(
        required(leg, "segment", &label)?,
        LEG_SEGMENTS,
        &format!("{}.segment", label),
    )?;
    let mut c = Map::new();
    c.insert("id".into(), json!(leg_id(leg, &label, index)?));
    c.insert("segment".into(), json!(segment));
    c.insert(
        "position".into(),
        json!(choice(
            required(leg, "position", &label)?,
            LEG_POSITIONS,
            &format!("{}.position", label)
        )?),
    );
    c.insert(
        "lots".into(),
        json!(integer(
            required(leg, "lots", &label)?,
            &format!("{}.lots", label),
            1,
            if segment == "cash" {
                MAX_CASH_QUANTITY
            } else {
                MAX_LOTS
            }
        )?),
    );
    if segment == "options" {
        option_fields(leg, &mut c, &label)?;
    } else {
        for f in ["option_type", "strike_mode", "atm_offset", "strike"] {
            if get(leg, f).is_some() {
                return Err(format!("{}.{} is only valid on an options leg", label, f));
            }
        }
    }
    if segment == "cash" {
        if get(leg, "expiry").is_some() {
            return Err(format!("{}.expiry is not valid on a cash leg", label));
        }
    } else {
        c.insert(
            "expiry".into(),
            json!(choice(
                required(leg, "expiry", &label)?,
                LEG_EXPIRIES,
                &format!("{}.expiry", label)
            )?),
        );
    }
    risk_fields(leg, &mut c, &label, &format!("{}.trail", label))?;
    Ok(Value::Object(c))
}

pub(super) fn validate_legs(raw: &Value, kind: &str, g: &SymbolGeneration) -> R<Vec<Value>> {
    let list = raw.as_array().ok_or("legs must be a list")?;
    if list.len() < MIN_LEGS {
        return Err(format!("A strategy needs at least {} leg", MIN_LEGS));
    }
    if list.len() > MAX_LEGS {
        return Err(format!(
            "A strategy takes at most {} legs, got {}",
            MAX_LEGS,
            list.len()
        ));
    }
    let legs = list
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if kind == "signal" {
                validate_signal_leg(l, i, g)
            } else {
                validate_leg(l, i)
            }
        })
        .collect::<R<Vec<_>>>()?;
    let mut ids: Vec<i64> = legs.iter().filter_map(|l| l["id"].as_i64()).collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != n {
        return Err("Every leg needs its own id".into());
    }
    Ok(legs)
}

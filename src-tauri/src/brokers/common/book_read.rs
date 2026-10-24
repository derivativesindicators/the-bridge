//! BR-03: a broker reply that cannot be read, or reads in the wrong shape,
//! is never an empty book, a flat position or nothing to cancel.
//!
//! A book is read only when the broker's own success marker is there and
//! its list is a list, every row of which reads. A missing, `null` or
//! mistyped list, and a row that does not read, make the whole read fail:
//! a row left out would hide an open order or a position. Where a broker
//! answers an empty book without a list (Angel, Motilal and mStock send
//! `data: null`, as the web's `test_position_read_failure` records), its
//! adapter reads that as empty itself; nowhere else does an absent list
//! mean an empty one.
//!
//! Outside a placement the error is the trader's to retry; inside one
//! (`outcome::placing`) it is uncertain, like every unmarked error. A
//! decision read (the open position a smart order sizes against, close
//! all) passes `position_read::unread` as its error instead.

use crate::error::{AppError, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The book a reply was meant to carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Book {
    Orders,
    Trades,
    Positions,
    Holdings,
    Funds,
}

impl Book {
    fn noun(self) -> &'static str {
        match self {
            Book::Orders => "order book",
            Book::Trades => "trade book",
            Book::Positions => "positions",
            Book::Holdings => "holdings",
            Book::Funds => "funds",
        }
    }
}

/// What a trader reads when `broker` answered without a readable `book`.
pub fn unreadable(broker: &str, book: Book) -> AppError {
    let name = crate::brokers::setup_guide::display_name(broker);
    tracing::warn!(
        broker,
        "{} answered without a readable {}",
        name,
        book.noun()
    );
    AppError::Broker(format!(
        "The Bridge could not read your {} from {}. Try again shortly; if this keeps happening, check them in your {} app.",
        book.noun(),
        name,
        name
    ))
}

/// The list at `key` of a success reply, or `unread()` when it is missing,
/// `null` or not a list.
pub fn list<'a>(v: &'a Value, key: &str, unread: impl Fn() -> AppError) -> Result<&'a [Value]> {
    match v.get(key) {
        Some(Value::Array(rows)) => Ok(rows),
        _ => Err(unread()),
    }
}

/// Every row of `rows` as `T`, or `unread()` when one does not read.
pub fn typed<T: DeserializeOwned>(rows: &[Value], unread: impl Fn() -> AppError) -> Result<Vec<T>> {
    rows.iter()
        .map(|r| {
            T::deserialize(r).map_err(|e| {
                tracing::warn!("An unreadable book row: {}", e);
                unread()
            })
        })
        .collect()
}

/// The object an answer carries at its figures (funds, a funds view):
/// anything but an object is `unreadable`, never a zero balance (BR-03).
pub fn object<'a>(v: Option<&'a Value>, broker: &str, book: Book) -> Result<&'a Value> {
    v.filter(|x| x.is_object())
        .ok_or_else(|| unreadable(broker, book))
}

/// A row's quantity as a broker sends it: a whole number, or text holding
/// one ("5", "-75", "10.0"). `None` when it is missing, `null`, empty, not
/// a number or not whole: that position cannot be read, and is never flat.
pub fn quantity(row: &Value, key: &str) -> Option<i64> {
    whole(row.get(key)?)
}

/// A whole number as a broker sends one: a number or text holding one;
/// `None` for anything else (`null`, empty, not a number, not whole).
pub fn whole(v: &Value) -> Option<i64> {
    let integral = |f: f64| (f.is_finite() && f.fract() == 0.0).then_some(f as i64);
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().and_then(integral)),
        Value::String(s) => {
            let t = s.trim();
            t.parse::<i64>()
                .ok()
                .or_else(|| t.parse::<f64>().ok().and_then(integral))
        }
        _ => None,
    }
}

/// `unread()` unless every row carries each of `keys` as a quantity
/// ([`quantity`]): a position row whose quantity cannot be read hides a
/// position, so the whole book is not read.
pub fn require_quantities(
    rows: &[Value],
    keys: &[&str],
    unread: impl Fn() -> AppError,
) -> Result<()> {
    for row in rows {
        if let Some(key) = keys.iter().find(|k| quantity(row, k).is_none()) {
            tracing::warn!("A position row without a readable {}", key);
            return Err(unread());
        }
    }
    Ok(())
}

/// Every row of a position list as `T`, each carrying `quantities`
/// ([`require_quantities`], then [`typed`]).
pub fn positions<T: DeserializeOwned>(
    rows: &[Value],
    quantities: &[&str],
    unread: impl Fn() -> AppError,
) -> Result<Vec<T>> {
    require_quantities(rows, quantities, &unread)?;
    typed(rows, unread)
}

/// The rows at `key` of a success reply as `T` ([`list`], then [`typed`]).
pub fn rows<T: DeserializeOwned>(
    v: &Value,
    key: &str,
    unread: impl Fn() -> AppError,
) -> Result<Vec<T>> {
    typed(list(v, key, &unread)?, unread)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Row {
        qty: i64,
    }

    fn unread() -> AppError {
        unreadable("fyers", Book::Positions)
    }

    #[test]
    fn only_a_list_of_readable_rows_is_a_book() {
        let ok = json!({"s": "ok", "netPositions": [{"qty": 5}]});
        assert_eq!(
            rows::<Row>(&ok, "netPositions", unread).unwrap(),
            vec![Row { qty: 5 }]
        );
        let empty = json!({"s": "ok", "netPositions": []});
        assert!(rows::<Row>(&empty, "netPositions", unread)
            .unwrap()
            .is_empty());
        for bad in [
            json!({"s": "ok"}),
            json!({"s": "ok", "netPositions": null}),
            json!({"s": "ok", "netPositions": "x"}),
            json!({"s": "ok", "netPositions": {}}),
            json!({"s": "ok", "netPositions": [{"qty": "five"}]}),
            json!({"s": "ok", "netPositions": [{"qty": 5}, {}]}),
            json!(null),
        ] {
            assert!(
                rows::<Row>(&bad, "netPositions", unread).is_err(),
                "{}",
                bad
            );
        }
        let row =
            json!({"a": 5, "b": "-75", "c": "10.0", "d": "abc", "e": null, "f": 1.5, "g": ""});
        assert_eq!(quantity(&row, "a"), Some(5));
        assert_eq!(quantity(&row, "b"), Some(-75));
        assert_eq!(quantity(&row, "c"), Some(10));
        for missing in ["d", "e", "f", "g", "h"] {
            assert_eq!(quantity(&row, missing), None, "{}", missing);
        }
        let rows = [json!({"q": 1, "r": "2"}), json!({"q": 0, "r": "0"})];
        assert!(require_quantities(&rows, &["q", "r"], unread).is_ok());
        assert!(require_quantities(&rows, &["q", "s"], unread).is_err());
        assert_eq!(
            unread().client_message(),
            "The Bridge could not read your positions from Fyers. Try again shortly; if this keeps happening, check them in your Fyers app."
        );
    }
}

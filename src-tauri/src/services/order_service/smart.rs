//! Smart orders (web `place_smart_order_service.py` and the brokers'
//! `place_smartorder_api`): read the open position, decide the order that
//! takes it to `position_size`, and place it, under the position's lock.
//!
//! Sizes are whole units, except on a CRYPTO order to the live broker,
//! which is decided and sent in exact decimals, the way `placeorder`
//! carries a fractional CRYPTO quantity (`Broker::place_order_exact`).
//! Everywhere else a fractional or oversized `position_size` is refused,
//! never truncated: a target of 0.5 read as 0 closed the whole position.
//! The web truncates it (`int(position_size)` in the Indian brokers and the
//! sandbox): a recorded deviation.

use super::{action, broker_error_reply, is_crypto, opt_dec, place_live, placed_event};
use super::{route_to_pending, Route, FRACTIONAL_REFUSED};
use crate::brokers::common::mapping::{Exchange, Product};
use crate::events::{Event, Mode};
use crate::services::core::{
    analyzer_request, broker_handle, f, meta, order_failed, publish, s, safe_request, BrokerHandle,
    Reply,
};
use crate::services::schemas::{MAX_BROKER_QUANTITY, POSITION_TOO_LARGE, QUANTITY_TOO_LARGE};
use crate::state::AppState;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::ops::{Neg, Sub};
use std::str::FromStr;
use std::sync::OnceLock;

/// What a smart order should do to reach `position_size`, in whole units
/// (`i64`) or, for live CRYPTO, exact sizes (`Decimal`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmartDecision<Q = i64> {
    Place { action: String, quantity: Q },
    NoAction(&'static str),
}

pub const NO_OPEN_POSITION: &str = "No OpenPosition Found. Not placing Exit order.";
pub const ALREADY_MATCHED: &str = "Positions Already Matched. No Action needed.";

/// The refusal of a fractional `position_size` where sizes are whole units.
pub const FRACTIONAL_POSITION_REFUSED: &str =
    "Fractional position sizes are accepted only for live CRYPTO orders. Use a whole-number position size.";

/// The refusal of a `position_size` or `quantity` that is not a plain number.
const SIZE_UNREADABLE: &str = "The Bridge could not read the position size or the quantity. Send each as a plain number, such as 2 or 0.5.";

/// The web's smart-order decision table (broker `place_smartorder_api`),
/// for whole units and exact sizes alike (`Q::default()` is zero).
pub fn smart_decision<Q>(current: Q, target: Q, quantity: Q, action: &str) -> SmartDecision<Q>
where
    Q: Copy + PartialOrd + Default + Sub<Output = Q> + Neg<Output = Q>,
{
    let zero = Q::default();
    let abs = |x: Q| if x < zero { -x } else { x };
    if target == zero && current == zero && quantity != zero {
        return SmartDecision::Place {
            action: action.to_ascii_uppercase(),
            quantity,
        };
    }
    if target == current {
        return SmartDecision::NoAction(if quantity == zero {
            NO_OPEN_POSITION
        } else {
            ALREADY_MATCHED
        });
    }
    let (a, q) = if target == zero && current > zero {
        ("SELL", abs(current))
    } else if target == zero && current < zero {
        ("BUY", abs(current))
    } else if current == zero {
        (if target > zero { "BUY" } else { "SELL" }, abs(target))
    } else if target > current {
        ("BUY", target - current)
    } else {
        ("SELL", current - target)
    };
    SmartDecision::Place {
        action: a.to_string(),
        quantity: q,
    }
}

// -- sizes ------------------------------------------------------------------

/// One size field as an exact decimal: a JSON number by its shortest text
/// (what the web's `float` holds), or numeric text; zero when absent (web
/// `data.get("position_size", "0")`); `None` when it is not a number.
fn exact_size(req: &Value, key: &str) -> Option<Decimal> {
    let text = match req.get(key) {
        None | Some(Value::Null) => return Some(Decimal::ZERO),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(t)) => t.trim().to_string(),
        Some(_) => return None,
    };
    Decimal::from_str(&text)
        .or_else(|_| Decimal::from_scientific(&text))
        .ok()
}

/// `position_size` and `quantity` exactly, as a CRYPTO order to the live
/// broker carries them.
fn exact_sizes(req: &Value) -> Result<(Decimal, Decimal), Reply> {
    // A JSON number too large for a decimal (1e30) is beyond any exchange.
    let read = |key: &str, too_large: &'static str| {
        exact_size(req, key).ok_or_else(|| {
            let huge = req
                .get(key)
                .and_then(Value::as_f64)
                .is_some_and(|x| x.abs() >= 1.0);
            Reply::error(400, if huge { too_large } else { SIZE_UNREADABLE })
        })
    };
    Ok((
        read("position_size", POSITION_TOO_LARGE)?,
        read("quantity", QUANTITY_TOO_LARGE)?,
    ))
}

/// `position_size` and `quantity` in whole units, as the sandbox and every
/// venue but CRYPTO take them: a fraction, or a size no exchange takes, is
/// refused before anything is read or sent.
fn whole_sizes(req: &Value) -> Result<(i64, i64), Reply> {
    let (target, quantity) = exact_sizes(req)?;
    if !target.fract().is_zero() {
        return Err(Reply::error(400, FRACTIONAL_POSITION_REFUSED));
    }
    if !quantity.fract().is_zero() {
        return Err(Reply::error(400, FRACTIONAL_REFUSED));
    }
    let units = |d: Decimal| d.to_i64().filter(|u| u.abs() <= MAX_BROKER_QUANTITY);
    match (units(target), units(quantity)) {
        (Some(target), Some(quantity)) => Ok((target, quantity)),
        (None, _) => Err(Reply::error(400, POSITION_TOO_LARGE)),
        (_, None) => Err(Reply::error(400, QUANTITY_TOO_LARGE)),
    }
}

/// A decided size as the order body carries it: whole units as a JSON
/// integer, a fractional CRYPTO size as its exact text (the web's Delta
/// smart order sends `str(quantity)`), which `place_live` parses for
/// `Broker::place_order_exact`.
fn order_size(q: Decimal) -> Value {
    match q.fract().is_zero().then(|| q.to_i64()).flatten() {
        Some(units) => json!(units),
        None => json!(q.normalize().to_string()),
    }
}

/// Striped locks so two smart orders on one position never both read the
/// same open quantity (bounded: a fixed number of stripes).
const STRIPES: usize = 64;

pub(crate) fn stripe(key: &str) -> &'static tokio::sync::Mutex<()> {
    static LOCKS: OnceLock<Vec<tokio::sync::Mutex<()>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| (0..STRIPES).map(|_| tokio::sync::Mutex::new(())).collect());
    let mut h: u64 = 1469598103934665603;
    for b in key.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(1099511628211);
    }
    &locks[(h % STRIPES as u64) as usize]
}

fn no_action_event(mode: Mode, req: &Value, request: Value, reply: &Reply) -> Event {
    Event::OrderNoAction {
        meta: meta(mode, "placesmartorder", request, &reply.body),
        symbol: s(req, "symbol"),
        exchange: s(req, "exchange"),
        message: reply.message(),
    }
}

/// `placesmartorder` in analyzer mode: the sandbox decides and places,
/// with the web's analyze-mode events.
async fn smart_order_in_sandbox(ctx: &AppState, req: &Value, target: i64, quantity: i64) -> Reply {
    let sreq = crate::sandbox::SmartOrderRequest {
        symbol: s(req, "symbol"),
        exchange: s(req, "exchange"),
        product: s(req, "product"),
        action: action(req),
        quantity,
        position_size: target,
        price: opt_dec(f(req, "price")),
        trigger_price: opt_dec(f(req, "trigger_price")),
        price_type: s(req, "pricetype"),
        strategy: s(req, "strategy"),
    };
    let request = analyzer_request(req, "placesmartorder");
    match ctx.sandbox.place_smart_order(sreq).await {
        Ok(crate::sandbox::replies::SmartOrderReply::Placed(p)) => {
            let reply = Reply::from_ser(&p);
            publish(
                ctx,
                placed_event(Mode::Analyze, "placesmartorder", req, request, &reply),
            );
            reply
        }
        Ok(crate::sandbox::replies::SmartOrderReply::NoAction(m)) => {
            let reply = Reply::from_ser(&m);
            publish(ctx, no_action_event(Mode::Analyze, req, request, &reply));
            reply
        }
        Err(e) => {
            let reply = Reply::sandbox(&e);
            publish(
                ctx,
                placed_event(Mode::Analyze, "placesmartorder", req, request, &reply),
            );
            reply
        }
    }
}

/// `placesmartorder`.
pub async fn place_smart_order(ctx: &AppState, req: &Value, route: Route) -> Reply {
    if let Some(r) = route_to_pending(ctx, "smartorder", req, route) {
        return r;
    }
    let analyze = match route.analyze(ctx) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if analyze {
        return match whole_sizes(req) {
            Ok((target, quantity)) => smart_order_in_sandbox(ctx, req, target, quantity).await,
            Err(refused) => refused,
        };
    }
    // Only a CRYPTO order to the live broker carries exact sizes.
    let sizes = if is_crypto(req) {
        exact_sizes(req)
    } else {
        whole_sizes(req).map(|(target, quantity)| (Decimal::from(target), Decimal::from(quantity)))
    };
    let (target, quantity) = match sizes {
        Ok(s) => s,
        Err(refused) => return refused,
    };
    let h = match broker_handle(ctx) {
        Ok(h) => h,
        Err(r) => return r,
    };
    smart_order_live(ctx, &h, req, target, quantity).await
}

/// `placesmartorder` on the live broker, in exact sizes (whole units unless
/// the order is CRYPTO): read the open position, decide and place, all
/// under the position's lock.
async fn smart_order_live(
    ctx: &AppState,
    h: &BrokerHandle,
    req: &Value,
    target: Decimal,
    quantity: Decimal,
) -> Reply {
    let symbol = s(req, "symbol");
    let (Ok(exchange), Ok(product)) = (
        s(req, "exchange").parse::<Exchange>(),
        s(req, "product").parse::<Product>(),
    ) else {
        return Reply::error(400, "Invalid exchange or product");
    };
    // Decide once, under the position's lock: read, decide, dispatch.
    let key = format!("{}:{}:{}", exchange, symbol, product);
    let _guard = stripe(&key).lock().await;
    let current = match h
        .broker
        .get_open_position_exact(&h.auth, &symbol, exchange, product)
        .await
    {
        Ok(q) => q,
        Err(e) => {
            let reply = broker_error_reply(&e, "Failed to place smart order due to internal error");
            order_failed(
                ctx,
                "placesmartorder",
                req,
                &reply,
                &symbol,
                exchange.as_str(),
            );
            return reply;
        }
    };
    match smart_decision(current, target, quantity, &action(req)) {
        SmartDecision::NoAction(msg) => {
            let reply = Reply::ok(json!({"status": "success", "message": msg}));
            publish(
                ctx,
                no_action_event(Mode::Live, req, safe_request(req), &reply),
            );
            reply
        }
        SmartDecision::Place { action, quantity } => {
            let mut order = req.clone();
            if let Some(m) = order.as_object_mut() {
                m.insert("action".into(), json!(action));
                m.insert("quantity".into(), order_size(quantity));
            }
            let reply = place_live(h, ctx, &order).await;
            if reply.is_success() {
                publish(
                    ctx,
                    placed_event(
                        Mode::Live,
                        "placesmartorder",
                        req,
                        safe_request(req),
                        &reply,
                    ),
                );
            } else {
                order_failed(
                    ctx,
                    "placesmartorder",
                    req,
                    &reply,
                    &symbol,
                    exchange.as_str(),
                );
            }
            reply
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(a: &str, q: i64) -> SmartDecision {
        SmartDecision::Place {
            action: a.into(),
            quantity: q,
        }
    }

    /// The web's smart-order table, row by row.
    #[test]
    fn smart_order_decision_table() {
        // (current, target, quantity, action) -> decision
        type Row = ((i64, i64, i64, &'static str), SmartDecision);
        let rows: Vec<Row> = vec![
            ((0, 0, 5, "buy"), place("BUY", 5)),
            ((0, 0, 0, "BUY"), SmartDecision::NoAction(NO_OPEN_POSITION)),
            ((5, 5, 0, "BUY"), SmartDecision::NoAction(NO_OPEN_POSITION)),
            (
                (-3, -3, 3, "SELL"),
                SmartDecision::NoAction(ALREADY_MATCHED),
            ),
            ((0, 10, 10, "BUY"), place("BUY", 10)),
            ((0, -3, 3, "SELL"), place("SELL", 3)),
            ((10, 15, 5, "BUY"), place("BUY", 5)),
            ((15, 5, 10, "SELL"), place("SELL", 10)),
            ((5, 0, 0, "SELL"), place("SELL", 5)),
            ((-4, 0, 0, "BUY"), place("BUY", 4)),
            ((-4, 2, 6, "BUY"), place("BUY", 6)),
            ((4, -2, 6, "SELL"), place("SELL", 6)),
        ];
        for ((c, t, q, a), want) in rows {
            assert_eq!(
                smart_decision(c, t, q, a),
                want,
                "current {} target {}",
                c,
                t
            );
        }
    }

    #[test]
    fn stripes_are_stable_and_bounded() {
        let a = stripe("NSE:SBIN:MIS") as *const _;
        let b = stripe("NSE:SBIN:MIS") as *const _;
        assert_eq!(a, b);
    }

    fn dec(x: &str) -> Decimal {
        Decimal::from_str(x).unwrap()
    }

    /// Live CRYPTO decides in exact sizes: a target of 0.5 is 0.5, never a
    /// 0 that closes the position.
    #[test]
    fn exact_sizes_decide_by_the_same_table() {
        let rows = [
            // (current, target, quantity) -> (action, quantity)
            (("6", "0.5", "0"), ("SELL", "5.5")),
            (("0.01", "0.5", "0"), ("BUY", "0.49")),
            (("0", "0.5", "1"), ("BUY", "0.5")),
            (("-0.3", "0.2", "0"), ("BUY", "0.5")),
            (("0.0105", "0", "0"), ("SELL", "0.0105")),
            (("0", "0", "0.25"), ("BUY", "0.25")),
        ];
        for ((c, t, q), (a, want)) in rows {
            assert_eq!(
                smart_decision(dec(c), dec(t), dec(q), "buy"),
                SmartDecision::Place {
                    action: a.into(),
                    quantity: dec(want)
                },
                "current {} target {}",
                c,
                t
            );
        }
        assert_eq!(
            smart_decision(dec("0.50"), dec("0.5"), dec("1"), "BUY"),
            SmartDecision::NoAction(ALREADY_MATCHED)
        );
    }

    #[test]
    fn whole_sizes_refuse_what_no_exchange_takes() {
        let req = |p: Value, q: Value| json!({"position_size": p, "quantity": q});
        let refused = |r: Result<(i64, i64), Reply>| {
            let r = r.unwrap_err();
            (r.status, r.message())
        };
        assert_eq!(whole_sizes(&req(json!(5), json!(2))).unwrap(), (5, 2));
        assert_eq!(whole_sizes(&req(json!(-3.0), json!(0))).unwrap(), (-3, 0));
        assert_eq!(whole_sizes(&json!({})).unwrap(), (0, 0));
        assert_eq!(
            refused(whole_sizes(&req(json!(0.5), json!(1)))),
            (400, FRACTIONAL_POSITION_REFUSED.to_string())
        );
        assert_eq!(
            refused(whole_sizes(&req(json!(5), json!(1.5)))),
            (400, FRACTIONAL_REFUSED.to_string())
        );
        for big in [
            json!(2_147_483_648u64),
            json!(-2_147_483_648i64),
            json!(1e30),
        ] {
            assert_eq!(
                refused(whole_sizes(&req(big, json!(1)))),
                (400, POSITION_TOO_LARGE.to_string())
            );
        }
        assert_eq!(
            refused(whole_sizes(&req(json!(1), json!(3e9)))),
            (400, QUANTITY_TOO_LARGE.to_string())
        );
        assert_eq!(
            refused(whole_sizes(&req(json!("half"), json!(1)))),
            (400, SIZE_UNREADABLE.to_string())
        );
    }

    #[test]
    fn exact_sizes_keep_the_digits_sent() {
        let (t, q) = exact_sizes(&json!({"position_size": 0.1, "quantity": "0.0005"})).unwrap();
        assert_eq!((t, q), (dec("0.1"), dec("0.0005")));
        let (t, _) = exact_sizes(&json!({"position_size": "1e-4", "quantity": 0})).unwrap();
        assert_eq!(t, dec("0.0001"));
        assert!(exact_sizes(&json!({"position_size": [1], "quantity": 0})).is_err());
        // Whole sizes go out as integers, fractions as their exact text.
        assert_eq!(order_size(dec("6")), json!(6));
        assert_eq!(order_size(dec("5.50")), json!("5.5"));
        assert_eq!(order_size(dec("0.49")), json!("0.49"));
    }
}

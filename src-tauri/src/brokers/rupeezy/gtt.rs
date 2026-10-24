//! GTT, single and OCO (web `api/gtt_api.py`, `mapping/gtt_data.py`,
//! https://vortex.rupeezy.in/docs/latest/gtt-order/).
//!
//! OpenAlgo names OCO legs by price: `triggerprice_sl` / `stoploss` is the
//! lower trigger, `triggerprice_tg` / `target` the higher. Vortex names them
//! by role, and which role is lower depends on the side: a SELL GTT (exiting
//! a long) has its stop-loss below and its profit above; a BUY GTT (exiting
//! a short) the reverse. Vortex triggers on its own price, so no last price
//! is needed.

use super::client::{order_path, Call};
use super::mapping::{
    from_vortex_quantity, oa_exchange, product, product_from_vortex, row_ticker, split_ticker,
    to_vortex_quantity,
};
use super::orders::{lot_of, rows_of};
use super::RupeezyBroker;
use crate::brokers::common::de::{f64_lenient, i64_lenient, string_lenient};
use crate::brokers::common::mapping::{Action, PriceType};
use crate::brokers::common::outcome::refusal_with_status;
use crate::brokers::common::symbols::{SymToken, SymbolResolver};
use crate::brokers::types::*;
use crate::error::{AppError, Result};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Map, Value};

const GTT: &str = "/trading/orders/gtt";

/// One leg of a GTT in the Vortex book. `id` and `trail` are echoed back on
/// a modify exactly as Vortex sent them.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct VortexGttLeg {
    pub id: Value,
    #[serde(deserialize_with = "i64_lenient")]
    pub quantity: i64,
    #[serde(deserialize_with = "f64_lenient")]
    pub price: f64,
    #[serde(deserialize_with = "f64_lenient")]
    pub trigger_price: f64,
    #[serde(deserialize_with = "string_lenient")]
    pub variety: String,
    #[serde(deserialize_with = "string_lenient")]
    pub status: String,
    #[serde(deserialize_with = "string_lenient")]
    pub transaction_type: String,
    pub trail: Option<Map<String, Value>>,
    #[serde(deserialize_with = "string_lenient")]
    pub created_at: String,
    #[serde(deserialize_with = "string_lenient")]
    pub updated_at: String,
    #[serde(deserialize_with = "string_lenient")]
    pub expires_at: String,
}

/// One GTT in the Vortex book.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct VortexGtt {
    #[serde(deserialize_with = "string_lenient")]
    pub id: String,
    #[serde(deserialize_with = "string_lenient")]
    pub ticker: String,
    #[serde(deserialize_with = "string_lenient")]
    pub exchange: String,
    #[serde(deserialize_with = "string_lenient")]
    pub symbol: String,
    #[serde(deserialize_with = "string_lenient")]
    pub product: String,
    #[serde(deserialize_with = "string_lenient")]
    pub transaction_type: String,
    #[serde(deserialize_with = "string_lenient")]
    pub trigger_type: String,
    #[serde(deserialize_with = "i64_lenient")]
    pub lot_size: i64,
    pub orders: Option<Vec<VortexGttLeg>>,
}

impl VortexGtt {
    /// The legs, lower trigger first.
    fn legs(&self) -> Vec<&VortexGttLeg> {
        let mut legs: Vec<&VortexGttLeg> = self.orders.iter().flatten().collect();
        legs.sort_by(|a, b| a.trigger_price.total_cmp(&b.trigger_price));
        legs
    }
}

/// Vortex GTT varieties are `RL` and `RL-MKT` only.
fn variety(p: PriceType) -> &'static str {
    if p == PriceType::Market {
        "RL-MKT"
    } else {
        "RL"
    }
}

fn leg(quantity: i64, price: f64, trigger: f64, variety: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("quantity".into(), json!(quantity));
    m.insert(
        "price".into(),
        json!(if variety == "RL-MKT" { 0.0 } else { price }),
    );
    m.insert("trigger_price".into(), json!(trigger));
    m.insert("variety".into(), json!(variety));
    m
}

/// A SINGLE trigger: `trigger_price`, else whichever OCO trigger is set
/// (the UI's modify route sends those).
fn single_trigger(req: &GttRequest) -> f64 {
    if req.trigger_price > 0.0 {
        req.trigger_price
    } else if req.triggerprice_sl > 0.0 {
        req.triggerprice_sl
    } else {
        req.triggerprice_tg
    }
}

/// The `POST /trading/orders/gtt` body (web `transform_place_gtt`).
pub(crate) fn place_body(req: &GttRequest, row: &SymToken) -> Result<Value> {
    let quantity = to_vortex_quantity(req.quantity, row.br_exchange(), lot_of(row))?;
    let v = variety(req.pricetype);
    let mut m = Map::new();
    m.insert("ticker".into(), json!(row_ticker(row)));
    m.insert("transaction_type".into(), json!(req.action.as_str()));
    m.insert("product".into(), json!(product(req.product)));
    match req.trigger_type {
        GttTriggerType::Oco => {
            let lower = leg(quantity, req.stoploss, req.triggerprice_sl, v);
            let higher = leg(quantity, req.target, req.triggerprice_tg, v);
            let (stoploss, profit) = match req.action {
                Action::Sell => (lower, higher),
                Action::Buy => (higher, lower),
            };
            m.insert("gtt_trigger_type".into(), json!("oco"));
            m.insert("stoploss".into(), Value::Object(stoploss));
            m.insert("profit".into(), Value::Object(profit));
        }
        GttTriggerType::Single => {
            m.insert("gtt_trigger_type".into(), json!("single"));
            m.extend(leg(quantity, req.price, single_trigger(req), v));
        }
    }
    Ok(Value::Object(m))
}

/// The `PUT` body (web `transform_modify_gtt`): one entry per existing leg,
/// matched by price order (the lower leg takes the new lower trigger), each
/// with its leg id and any trailing stop it had. Vortex changes the legs a
/// GTT has, so SINGLE cannot become OCO or back.
pub(crate) fn modify_body(req: &GttRequest, existing: &VortexGtt, row: &SymToken) -> Result<Value> {
    let quantity = to_vortex_quantity(req.quantity, row.br_exchange(), lot_of(row))?;
    let v = variety(req.pricetype);
    let legs = existing.legs();
    let existing_oco = legs.len() >= 2;
    if (req.trigger_type == GttTriggerType::Oco) != existing_oco {
        return Err(AppError::Validation(
            "A GTT cannot be changed between SINGLE and OCO. Cancel it and place a new one instead."
                .into(),
        ));
    }
    let targets = if existing_oco {
        vec![
            (req.stoploss, req.triggerprice_sl),
            (req.target, req.triggerprice_tg),
        ]
    } else {
        vec![(req.price, single_trigger(req))]
    };
    if legs.len() != targets.len() {
        return Err(AppError::Validation(
            "This GTT has legs The Bridge cannot change. Cancel it and place a new one instead."
                .into(),
        ));
    }
    let body = legs
        .iter()
        .zip(targets)
        .map(|(l, (price, trigger))| {
            let mut e = Map::new();
            e.insert("id".into(), l.id.clone());
            e.extend(leg(quantity, price, trigger, v));
            if let Some(t) = l.trail.as_ref().filter(|t| !t.is_empty()) {
                let field = |k: &str| t.get(k).cloned().unwrap_or(Value::Null);
                e.insert(
                    "trail".into(),
                    json!({
                        "id": field("id"),
                        "trail_jump_point": field("trail_jump_point"),
                        "trail_jump_type": field("trail_jump_type"),
                    }),
                );
            }
            Value::Object(e)
        })
        .collect();
    Ok(Value::Array(body))
}

/// One status for a GTT: active while any leg can still fire, then
/// triggered (in a fired OCO the sibling only shows cancelled).
fn gtt_status(legs: &[&VortexGttLeg]) -> String {
    let statuses: Vec<String> = legs.iter().map(|l| l.status.to_lowercase()).collect();
    ["active", "triggered"]
        .iter()
        .find(|s| statuses.iter().any(|x| x == *s))
        .map(|s| s.to_string())
        .or_else(|| statuses.first().cloned())
        .unwrap_or_default()
}

/// One Vortex GTT -> one OpenAlgo GTT row.
fn gtt_row(
    g: &VortexGtt,
    legs: &[&VortexGttLeg],
    status: String,
    symbols: &SymbolResolver,
) -> GttOrder {
    let exchange = oa_exchange(&g.exchange);
    let brsymbol = match split_ticker(&g.ticker) {
        "" => g.symbol.as_str(),
        s => s,
    };
    let product = match product_from_vortex(&exchange, &g.product) {
        p if p.is_empty() => g.product.clone(),
        p => p,
    };
    let (created_at, updated_at, expires_at) = legs
        .first()
        .map(|l| {
            (
                l.created_at.clone(),
                l.updated_at.clone(),
                l.expires_at.clone(),
            )
        })
        .unwrap_or_default();
    GttOrder {
        trigger_id: g.id.clone(),
        trigger_type: if g.trigger_type.eq_ignore_ascii_case("oco") {
            "two-leg"
        } else {
            "single"
        }
        .into(),
        status,
        symbol: symbols.oa_symbol_or_raw(brsymbol, &exchange),
        trigger_prices: legs.iter().map(|l| l.trigger_price).collect(),
        last_price: 0.0,
        legs: legs
            .iter()
            .map(|l| GttLeg {
                action: if l.transaction_type.is_empty() {
                    &g.transaction_type
                } else {
                    &l.transaction_type
                }
                .to_ascii_uppercase(),
                quantity: from_vortex_quantity(l.quantity, &g.exchange, g.lot_size),
                price: l.price,
                pricetype: if l.variety == "RL-MKT" {
                    "MARKET"
                } else {
                    "LIMIT"
                }
                .into(),
                product: product.clone(),
            })
            .collect(),
        created_at,
        updated_at,
        expires_at,
        exchange,
    }
}

/// The Vortex GTT book -> OpenAlgo GTT rows (web `map_gtt_book`); finished
/// GTTs only with `include_history`.
pub(crate) fn map_gtt_book(
    rows: &[VortexGtt],
    symbols: &SymbolResolver,
    include_history: bool,
) -> Vec<GttOrder> {
    rows.iter()
        .filter_map(|g| {
            let legs = g.legs();
            let status = gtt_status(&legs);
            (include_history || status == "active").then(|| gtt_row(g, &legs, status, symbols))
        })
        .collect()
}

fn require_id(trigger_id: &str) -> Result<()> {
    if trigger_id.trim().is_empty() {
        return Err(AppError::Validation("trigger_id is required".into()));
    }
    Ok(())
}

fn id_of(v: &Value) -> String {
    match v.get("data").and_then(|d| d.get("order_id")) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

pub async fn place_gtt(
    b: &RupeezyBroker,
    auth: &AuthToken,
    req: &GttRequest,
) -> Result<GttResponse> {
    let body = place_body(req, &b.instrument(&req.key)?)?;
    let a = b
        .call(auth, Call::write(Method::POST, GTT, Some(&body)))
        .await?;
    if !a.succeeded() {
        let msg = a.message();
        tracing::warn!(status = a.status.as_u16(), "Rupeezy refused a GTT: {}", msg);
        return Err(refusal_with_status(
            a.status.as_u16(),
            if msg.is_empty() {
                "Rupeezy did not accept the GTT.".to_string()
            } else {
                msg
            },
        ));
    }
    let id = id_of(&a.body);
    if id.is_empty() {
        tracing::warn!("Rupeezy accepted a GTT without an id");
        return Err(AppError::Broker(
            "Rupeezy accepted the GTT but returned no id. Check the GTT book before placing it again."
                .into(),
        ));
    }
    Ok(GttResponse { trigger_id: id })
}

async fn gtt_book(b: &RupeezyBroker, auth: &AuthToken) -> Result<Vec<VortexGtt>> {
    let v = b.ok(auth, Call::get(GTT)).await?;
    Ok(rows_of(&v, &["data"]).unwrap_or_default())
}

/// Vortex changes a GTT leg by leg, each by its leg id, so the GTT is read
/// from the book first.
pub async fn modify_gtt(
    b: &RupeezyBroker,
    auth: &AuthToken,
    trigger_id: &str,
    req: &GttRequest,
) -> Result<GttResponse> {
    require_id(trigger_id)?;
    let row = b.instrument(&req.key)?;
    let existing = gtt_book(b, auth)
        .await?
        .into_iter()
        .find(|g| g.id == trigger_id)
        .ok_or_else(|| AppError::NotFound(format!("GTT {} was not found.", trigger_id)))?;
    let body = modify_body(req, &existing, &row)?;
    let a = b
        .call(
            auth,
            Call::write(Method::PUT, &order_path(GTT, trigger_id), Some(&body)),
        )
        .await?;
    if !a.succeeded() {
        return Err(a.refused("a GTT change"));
    }
    Ok(GttResponse {
        trigger_id: trigger_id.to_string(),
    })
}

pub async fn cancel_gtt(
    b: &RupeezyBroker,
    auth: &AuthToken,
    trigger_id: &str,
) -> Result<GttResponse> {
    require_id(trigger_id)?;
    let a = b
        .call(
            auth,
            Call::write(Method::DELETE, &order_path(GTT, trigger_id), None),
        )
        .await?;
    if !a.succeeded() {
        return Err(a.refused("a GTT cancel"));
    }
    Ok(GttResponse {
        trigger_id: trigger_id.to_string(),
    })
}

pub async fn get_gtt_book(
    b: &RupeezyBroker,
    auth: &AuthToken,
    include_history: bool,
) -> Result<Vec<GttOrder>> {
    Ok(map_gtt_book(
        &gtt_book(b, auth).await?,
        b.resolver(),
        include_history,
    ))
}

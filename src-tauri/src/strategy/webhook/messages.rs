//! The Bridge 1.0.7: plain-text alert bodies for signal strategies.
//!
//! TradingView `alert()` calls and many indicators send a bare message such
//! as `BUY` or `CLOSE LONG: Normal Exit`, not JSON. A signal strategy maps
//! such a message to one of its four actions through `webhook_messages`, a
//! list of rules `{match, action, prefix}`; `null` means the defaults below.
//! Matching trims the text and ignores case; an exact rule wins over a
//! prefix rule, and the longest prefix wins among prefixes. The URL token
//! stays the only credential: nothing in the text authenticates.

use serde_json::{json, Value};

/// Most rules one strategy may carry.
pub const MAX_RULES: usize = 20;
/// Longest text a rule may match.
pub const MAX_MATCH_LEN: usize = 100;

/// The defaults: Arko's Strategy's messages and the common BUY / SELL.
pub fn default_rules() -> Value {
    json!([
        {"match": "BUY", "action": "long_entry", "prefix": false},
        {"match": "SELL", "action": "short_entry", "prefix": false},
        {"match": "CLOSE LONG", "action": "long_exit", "prefix": true},
        {"match": "CLOSE SHORT", "action": "short_exit", "prefix": true},
    ])
}

/// Validate a strategy's `webhook_messages` (absent or null: defaults).
pub fn validate_rules(v: Option<&Value>) -> Result<Value, String> {
    let label = "webhook_messages";
    let rules = match v {
        None | Some(Value::Null) => return Ok(Value::Null),
        Some(Value::Array(a)) => a,
        Some(_) => return Err(format!("{} must be a list of rules", label)),
    };
    if rules.len() > MAX_RULES {
        return Err(format!("{} may hold at most {} rules", label, MAX_RULES));
    }
    let mut out = Vec::with_capacity(rules.len());
    let mut seen: Vec<(String, bool)> = Vec::new();
    for (i, r) in rules.iter().enumerate() {
        let at = format!("{}[{}]", label, i);
        let Some(m) = r.as_object() else {
            return Err(format!("{} must be an object", at));
        };
        for k in m.keys() {
            if !["match", "action", "prefix"].contains(&k.as_str()) {
                return Err(format!("{}.{} is not a known field", at, k));
            }
        }
        let text = m
            .get("match")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if text.is_empty() {
            return Err(format!("{}.match must be some text", at));
        }
        if text.chars().count() > MAX_MATCH_LEN {
            return Err(format!(
                "{}.match is longer than {} characters",
                at, MAX_MATCH_LEN
            ));
        }
        let action = m
            .get("action")
            .and_then(Value::as_str)
            .map(|a| a.trim().to_ascii_lowercase())
            .unwrap_or_default();
        if !crate::strategy::signals::SIGNAL_ACTIONS.contains(&action.as_str()) {
            return Err(format!(
                "{}.action must be one of {}",
                at,
                crate::strategy::signals::SIGNAL_ACTIONS.join(", ")
            ));
        }
        let prefix = match m.get("prefix") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(format!("{}.prefix must be true or false", at)),
        };
        let key = (text.to_uppercase(), prefix);
        if seen.contains(&key) {
            return Err(format!("{}: '{}' is listed twice", at, text));
        }
        seen.push(key);
        out.push(json!({"match": text, "action": action, "prefix": prefix}));
    }
    Ok(Value::Array(out))
}

/// The action a plain-text body maps to under `rules` (null: defaults).
pub fn action_for(text: &str, rules: &Value) -> Option<String> {
    let defaults = default_rules();
    let empty = Vec::new();
    let rules = match rules {
        Value::Array(a) => a,
        _ => defaults.as_array().unwrap_or(&empty),
    };
    let wanted = text.trim().to_uppercase();
    if wanted.is_empty() {
        return None;
    }
    let field = |r: &Value, k: &str| r[k].as_str().unwrap_or("").trim().to_uppercase();
    if let Some(r) = rules
        .iter()
        .find(|r| !r["prefix"].as_bool().unwrap_or(false) && field(r, "match") == wanted)
    {
        return r["action"].as_str().map(str::to_string);
    }
    rules
        .iter()
        .filter(|r| r["prefix"].as_bool().unwrap_or(false))
        .filter(|r| {
            let m = field(r, "match");
            !m.is_empty() && wanted.starts_with(&m)
        })
        .max_by_key(|r| field(r, "match").len())
        .and_then(|r| r["action"].as_str().map(str::to_string))
}

/// The text of a body that is not a JSON object: UTF-8, trimmed, a JSON
/// string's quotes removed. `None` when it is empty or not text.
pub fn body_text(body: &[u8]) -> Option<String> {
    let t = std::str::from_utf8(body).ok()?.trim();
    let t = match serde_json::from_str::<Value>(t) {
        Ok(Value::String(s)) => s,
        _ => t.to_string(),
    };
    let t = t.trim().to_string();
    (!t.is_empty()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_map_arkos_messages() {
        let d = Value::Null;
        assert_eq!(action_for("BUY", &d).as_deref(), Some("long_entry"));
        assert_eq!(action_for("  sell \n", &d).as_deref(), Some("short_entry"));
        assert_eq!(
            action_for("CLOSE LONG: Normal Exit", &d).as_deref(),
            Some("long_exit")
        );
        assert_eq!(
            action_for("CLOSE LONG: Trailing Profit", &d).as_deref(),
            Some("long_exit")
        );
        assert_eq!(
            action_for("close short: Normal Exit", &d).as_deref(),
            Some("short_exit")
        );
        assert_eq!(
            action_for("CLOSE SHORT: Trailing Profit", &d).as_deref(),
            Some("short_exit")
        );
        // Exact rules do not match longer text.
        assert_eq!(action_for("BUY NOW", &d), None);
        assert_eq!(action_for("", &d), None);
    }

    #[test]
    fn custom_rules_exact_beats_prefix_and_longest_prefix_wins() {
        let rules = validate_rules(Some(&json!([
            {"match": "LONG", "action": "long_entry", "prefix": true},
            {"match": "LONG EXIT", "action": "long_exit", "prefix": true},
            {"match": "long entry", "action": "short_entry"},
        ])))
        .unwrap();
        assert_eq!(
            action_for("LONG ENTRY", &rules).as_deref(),
            Some("short_entry")
        );
        assert_eq!(
            action_for("Long exit (stop)", &rules).as_deref(),
            Some("long_exit")
        );
        assert_eq!(
            action_for("LONG something", &rules).as_deref(),
            Some("long_entry")
        );
        assert_eq!(action_for("BUY", &rules), None);
    }

    #[test]
    fn rules_are_validated() {
        assert_eq!(validate_rules(None).unwrap(), Value::Null);
        assert!(validate_rules(Some(&json!({}))).is_err());
        assert!(validate_rules(Some(&json!([{"match": "", "action": "long_entry"}]))).is_err());
        assert!(validate_rules(Some(&json!([{"match": "X", "action": "buy"}]))).is_err());
        assert!(validate_rules(Some(
            &json!([{"match": "X", "action": "long_entry", "extra": 1}])
        ))
        .is_err());
        assert!(validate_rules(Some(&json!([
            {"match": "X", "action": "long_entry"}, {"match": "x", "action": "short_entry"}
        ])))
        .is_err());
        let many: Vec<Value> = (0..21)
            .map(|i| json!({"match": format!("M{}", i), "action": "long_entry"}))
            .collect();
        assert!(validate_rules(Some(&Value::Array(many))).is_err());
        let ok =
            validate_rules(Some(&json!([{"match": " Buy ", "action": "LONG_ENTRY"}]))).unwrap();
        assert_eq!(
            ok,
            json!([{"match": "Buy", "action": "long_entry", "prefix": false}])
        );
    }

    #[test]
    fn body_text_reads_plain_and_quoted() {
        assert_eq!(body_text(b"  BUY \n").as_deref(), Some("BUY"));
        assert_eq!(body_text(b"\"SELL\"").as_deref(), Some("SELL"));
        assert_eq!(body_text(b"   "), None);
        assert_eq!(body_text(&[0xff, 0xfe]), None);
    }
}

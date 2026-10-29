//! Ollama Cloud quota balances from <https://docs.ollama.com/api/balance>.
//! Percentages represent consumption, not the API's legacy remaining percentage.

use serde_json::Value;

use crate::time::parse_rfc3339;
use crate::usage::{SubscriptionUsage, UsageLimit, UsageStatus};

pub const MONTHLY_KEY: &str = "monthly";
pub const SESSION_KEY: &str = "session";
pub const WEEKLY_KEY: &str = "weekly";

/// Parses current monthly credit balances and legacy session/weekly quotas.
/// Missing or unavailable numbers never produce an invented percentage.
pub fn parse_usage(json: &str, fetched_at: i64) -> Result<SubscriptionUsage, String> {
    let root: Value = serde_json::from_str(json).map_err(|_| "invalid balance JSON")?;
    let root = root.as_object().ok_or("balance response is not an object")?;
    let included = root.get("included").ok_or("balance response has no included balance")?;
    if !included.is_null() && !included.is_object() {
        return Err("included balance is not an object".into());
    }
    let purchased = root.get("purchased").and_then(|v| v.get("balance_usd")).and_then(number);
    let mut limits = Vec::new();
    if let (Some(balance), Some(allowance)) =
        (included.get("balance_usd").and_then(number), included.get("allowance_usd").and_then(number))
    {
        if allowance > 0.0 {
            let used = allowance - balance;
            let utilization = used / allowance * 100.0;
            if utilization.is_finite() {
                let mut detail = format!("${used:.2} / ${allowance:.2} used");
                if let Some(purchased) = purchased {
                    detail.push_str(&format!("; ${purchased:.2} purchased remaining"));
                }
                limits.push(UsageLimit {
                    key: MONTHLY_KEY,
                    utilization,
                    resets_at: included
                        .get("period")
                        .and_then(|v| v.get("until"))
                        .and_then(Value::as_str)
                        .and_then(parse_rfc3339),
                    detail: Some(detail),
                });
            }
        }
    }
    for key in [SESSION_KEY, WEEKLY_KEY] {
        let Some(entry) = included.get(key) else { continue };
        let Some(remaining) = entry.get("remaining_percent").and_then(number) else { continue };
        if !(0.0..=100.0).contains(&remaining) {
            continue;
        }
        limits.push(UsageLimit {
            key,
            utilization: 100.0 - remaining,
            resets_at: entry.get("resets_at").and_then(Value::as_str).and_then(parse_rfc3339),
            detail: purchased.map(|balance| format!("${balance:.2} purchased remaining")),
        });
    }
    Ok(SubscriptionUsage { status: UsageStatus::Ok, error: None, limits, fetched_at: Some(fetched_at), subscription: None })
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monthly_consumption_and_purchased_balance() {
        let usage = parse_usage(r#"{"included":{"balance_usd":72.5,"allowance_usd":100,"period":{"until":"2026-10-15T09:30:00Z"}},"purchased":{"balance_usd":25}}"#, 123).unwrap();
        let limit = usage.get(MONTHLY_KEY).unwrap();
        assert!((limit.utilization - 27.5).abs() < 1e-10);
        assert_eq!(limit.resets_at, parse_rfc3339("2026-10-15T09:30:00Z"));
        assert_eq!(usage.fetched_at, Some(123));
    }

    #[test]
    fn legacy_remaining_is_inverted() {
        let usage = parse_usage(r#"{"included":{"session":{"remaining_percent":75,"resets_at":"2026-10-01T07:00:00Z"},"weekly":{"remaining_percent":40,"resets_at":"2026-10-05T00:00:00Z"}}}"#, 0).unwrap();
        assert_eq!(usage.get(SESSION_KEY).unwrap().utilization, 25.0);
        assert_eq!(usage.get(WEEKLY_KEY).unwrap().utilization, 60.0);
        assert_eq!(usage.get(SESSION_KEY).unwrap().resets_at, parse_rfc3339("2026-10-01T07:00:00Z"));
    }

    #[test]
    fn zero_allowance_and_unavailable_values_have_no_percentage() {
        for json in [
            r#"{"included":{"balance_usd":0,"allowance_usd":0}}"#,
            r#"{"included":{"balance_usd":null,"allowance_usd":100}}"#,
            r#"{"included":null}"#,
            r#"{"included":{"session":{"remaining_percent":null},"weekly":null}}"#,
        ] {
            assert!(parse_usage(json, 0).unwrap().limits.is_empty());
        }
    }

    #[test]
    fn malformed_data_is_sanitized_or_omitted() {
        for json in ["not-json-secret", "[]", "{}", r#"{"included":"secret"}"#] {
            let error = parse_usage(json, 0).unwrap_err();
            assert!(!error.contains("secret"));
        }
        let usage = parse_usage(r#"{"included":{"balance_usd":"72.5","allowance_usd":100,"session":{"remaining_percent":101},"weekly":{"remaining_percent":-1}}}"#, 0).unwrap();
        assert!(usage.limits.is_empty());
    }
}

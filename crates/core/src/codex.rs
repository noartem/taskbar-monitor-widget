//! Read-only Codex ChatGPT credentials and subscription quota parsing.
//!
//! Schema: openai/codex `login/src/{auth/storage,token_data}.rs` and
//! `codex-backend-openapi-models/src/models/rate_limit_{status_payload,status_details,window_snapshot}.rs`.
//! Window identity comes from its duration, never its primary/secondary position.

use std::path::PathBuf;

use serde_json::Value;

use crate::usage::{SubscriptionUsage, UsageLimit, UsageStatus};

pub const SESSION_KEY: &str = "session";
pub const WEEKLY_KEY: &str = "weekly";
pub const CREDENTIALS_FILE: &str = "auth.json";

/// Only ChatGPT OAuth credentials are accepted; API keys are not subscription credentials.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub access_token: String,
    pub account_id: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("access_token", &"<redacted>")
            .field("account_id", &self.account_id.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Reads tokens only. Never refreshes tokens or changes Codex's credentials file.
pub fn parse_credentials(json: &str) -> Option<Credentials> {
    let root: Value = serde_json::from_str(json).ok()?;
    match root.get("auth_mode") {
        None | Some(Value::Null) => {}
        Some(Value::String(mode)) if mode == "chatgpt" || mode == "chatgptAuthTokens" => {}
        _ => return None,
    }
    let tokens = root.get("tokens")?.as_object()?;
    let access_token = tokens.get("access_token")?.as_str()?;
    if !header_identifier(access_token) {
        return None;
    }
    let account_id = match tokens.get("account_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) if id.is_empty() => None,
        Some(Value::String(id)) if header_identifier(id) => Some(id.clone()),
        _ => return None,
    };
    Some(Credentials { access_token: access_token.to_owned(), account_id })
}

// Neither credential may introduce another header into the shared WinHTTP request.
fn header_identifier(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_graphic())
}

/// `$CODEX_HOME/auth.json`, or `~/.codex/auth.json` when CODEX_HOME is unset.
pub fn default_credentials_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("CODEX_HOME").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir).join(CREDENTIALS_FILE);
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).unwrap_or_default();
    PathBuf::from(home).join(".codex").join(CREDENTIALS_FILE)
}

/// Accepts a file or an existing directory containing `auth.json`; empty means default.
pub fn resolve_credentials_path(configured: &str) -> PathBuf {
    let configured = configured.trim().trim_matches('"');
    if configured.is_empty() {
        return default_credentials_path();
    }
    let path = PathBuf::from(configured);
    if path.is_dir() { path.join(CREDENTIALS_FILE) } else { path }
}

/// Parses percent USED and Unix reset seconds, without echoing response data in errors.
/// Null/absent quota windows are valid for accounts without an active limit.
pub fn parse_usage(json: &str, fetched_at: i64) -> Result<SubscriptionUsage, String> {
    let root: Value = serde_json::from_str(json).map_err(|_| "invalid usage JSON")?;
    let root = root.as_object().ok_or("usage response is not an object")?;
    if !root.contains_key("rate_limit") && !root.contains_key("plan_type") {
        return Err("usage response has no subscription fields".into());
    }
    let subscription = match root.get("plan_type") {
        None | Some(Value::Null) => None,
        Some(Value::String(plan)) => Some(plan.clone()),
        _ => return Err("invalid subscription plan".into()),
    };
    let mut limits = Vec::new();
    match root.get("rate_limit") {
        None | Some(Value::Null) => {}
        Some(Value::Object(rate_limit)) => {
            for slot in ["primary_window", "secondary_window"] {
                let Some(window) = rate_limit.get(slot).filter(|v| !v.is_null()) else { continue };
                let window = window.as_object().ok_or("invalid quota window")?;
                let utilization = window.get("used_percent").and_then(Value::as_f64).ok_or("invalid used percentage")?;
                if !utilization.is_finite() || utilization < 0.0 {
                    return Err("invalid used percentage".into());
                }
                let duration = window.get("limit_window_seconds").and_then(Value::as_i64).ok_or("invalid quota duration")?;
                let key = match duration {
                    18_000 => SESSION_KEY,
                    604_800 => WEEKLY_KEY,
                    // An unknown duration must never be presented as a five-hour or weekly limit.
                    _ if duration > 0 => continue,
                    _ => return Err("invalid quota duration".into()),
                };
                let resets_at = match optional_seconds(window.get("reset_at"))? {
                    Some(reset) => Some(reset),
                    None => optional_seconds(window.get("reset_after_seconds"))?
                        .map(|seconds| fetched_at.checked_add(seconds).ok_or("reset time overflow"))
                        .transpose()?,
                };
                limits.push(UsageLimit { key, utilization, resets_at, detail: None });
            }
        }
        _ => return Err("invalid rate limit data".into()),
    }
    limits.sort_unstable_by_key(|limit| if limit.key == SESSION_KEY { 0 } else { 1 });
    Ok(SubscriptionUsage { status: UsageStatus::Ok, error: None, limits, fetched_at: Some(fetched_at), subscription })
}

fn optional_seconds(value: Option<&Value>) -> Result<Option<i64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_i64().filter(|seconds| *seconds >= 0).map(Some).ok_or_else(|| "invalid reset time".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swapped_windows_use_duration_and_preserve_used_percent() {
        let usage = parse_usage(
            r#"{"plan_type":"plus","rate_limit":{
                "primary_window":{"used_percent":17.5,"limit_window_seconds":604800,"reset_at":1900000000},
                "secondary_window":{"used_percent":42,"limit_window_seconds":18000,"reset_at":1800000000}
            }}"#,
            100,
        )
        .unwrap();
        assert_eq!(usage.status, UsageStatus::Ok);
        assert_eq!(usage.subscription.as_deref(), Some("plus"));
        assert_eq!(usage.fetched_at, Some(100));
        assert_eq!(usage.get(SESSION_KEY).unwrap().utilization, 42.0);
        assert_eq!(usage.get(SESSION_KEY).unwrap().resets_at, Some(1_800_000_000));
        assert_eq!(usage.get(WEEKLY_KEY).unwrap().utilization, 17.5);
        assert_eq!(usage.get(WEEKLY_KEY).unwrap().resets_at, Some(1_900_000_000));
    }

    #[test]
    fn weekly_only_primary_is_not_session() {
        let usage = parse_usage(
            r#"{"rate_limit":{"primary_window":{"used_percent":80,"limit_window_seconds":604800},"secondary_window":null}}"#,
            0,
        )
        .unwrap();
        assert!(usage.get(SESSION_KEY).is_none());
        assert_eq!(usage.get(WEEKLY_KEY).unwrap().utilization, 80.0);
        assert_eq!(usage.get(WEEKLY_KEY).unwrap().resets_at, None);
    }

    #[test]
    fn unknown_duration_is_not_mislabeled() {
        let usage =
            parse_usage(r#"{"rate_limit":{"primary_window":{"used_percent":3,"limit_window_seconds":3600}}}"#, 0).unwrap();
        assert!(usage.limits.is_empty());
    }

    #[test]
    fn relative_reset_uses_fetch_time_and_absolute_reset_wins() {
        let usage = parse_usage(
            r#"{"rate_limit":{
                "primary_window":{"used_percent":0,"limit_window_seconds":18000,"reset_at":null,"reset_after_seconds":60},
                "secondary_window":{"used_percent":0,"limit_window_seconds":604800,"reset_at":500,"reset_after_seconds":99}
            }}"#,
            100,
        )
        .unwrap();
        assert_eq!(usage.get(SESSION_KEY).unwrap().resets_at, Some(160));
        assert_eq!(usage.get(WEEKLY_KEY).unwrap().resets_at, Some(500));
        assert!(
            parse_usage(
                r#"{"rate_limit":{"primary_window":{"used_percent":0,"limit_window_seconds":18000,"reset_after_seconds":1}}}"#,
                i64::MAX,
            )
            .is_err()
        );
    }

    #[test]
    fn absent_and_null_limits_are_valid_but_malformed_data_is_not() {
        for json in [
            r#"{"plan_type":"pro"}"#,
            r#"{"rate_limit":null}"#,
            r#"{"rate_limit":{}}"#,
            r#"{"rate_limit":{"primary_window":null,"secondary_window":null}}"#,
        ] {
            let usage = parse_usage(json, 50).unwrap();
            assert_eq!(usage.status, UsageStatus::Ok);
            assert!(usage.limits.is_empty());
        }
        for json in [
            "not JSON",
            "null",
            "[]",
            "{}",
            r#"{"rate_limit":[]}"#,
            r#"{"plan_type":false}"#,
            r#"{"rate_limit":{"primary_window":false}}"#,
            r#"{"rate_limit":{"primary_window":{"used_percent":null,"limit_window_seconds":18000}}}"#,
            r#"{"rate_limit":{"primary_window":{"used_percent":"private-value","limit_window_seconds":18000}}}"#,
            r#"{"rate_limit":{"primary_window":{"used_percent":0,"limit_window_seconds":18000,"reset_at":"private-value"}}}"#,
        ] {
            let error = parse_usage(json, 0).unwrap_err();
            assert!(!error.contains("private-value"));
        }
    }

    #[test]
    fn accepts_chatgpt_credentials_and_redacts_debug() {
        let credentials = parse_credentials(
            r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"access_token":"secret-access-token","account_id":"private-account","refresh_token":"unused"}}"#,
        )
        .unwrap();
        assert_eq!(credentials.access_token, "secret-access-token");
        assert_eq!(credentials.account_id.as_deref(), Some("private-account"));
        let debug = format!("{credentials:?}");
        assert!(!debug.contains("secret-access-token"));
        assert!(!debug.contains("private-account"));
        assert!(parse_credentials(r#"{"tokens":{"access_token":"token"}}"#).unwrap().account_id.is_none());
        assert!(
            parse_credentials(r#"{"auth_mode":"chatgptAuthTokens","tokens":{"access_token":"token","account_id":null}}"#)
                .is_some()
        );
    }

    #[test]
    fn rejects_api_keys_invalid_credentials_and_header_injection() {
        for json in [
            r#"{"OPENAI_API_KEY":"sk-api-key"}"#,
            r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-api-key","tokens":{"access_token":"old-oauth-token"}}"#,
            r#"{"tokens":{"access_token":""}}"#,
            r#"{"tokens":{"access_token":"   "}}"#,
            r#"{"tokens":{"access_token":7}}"#,
            r#"{"tokens":{"access_token":"token\r\nInjected: value"}}"#,
            r#"{"tokens":{"access_token":"token","account_id":"account\nInjected: value"}}"#,
            r#"{"tokens":{"access_token":"token","account_id":42}}"#,
            "{}",
            "not JSON",
        ] {
            assert!(parse_credentials(json).is_none());
        }
    }

    #[test]
    fn configured_credentials_accept_file_or_directory() {
        assert_eq!(resolve_credentials_path(" \"custom-auth.json\" "), PathBuf::from("custom-auth.json"));
        assert_eq!(resolve_credentials_path("."), PathBuf::from(".").join(CREDENTIALS_FILE));
    }
}

//! Claude Code subscription usage: credentials file, `/api/oauth/usage` response, state merging.
//!
//! The usage endpoint is not a documented public API, so parsing is lenient: every top-level object with a
//! numeric `utilization` becomes a [`ClaudeLimit`]. New per-model weekly limits (`seven_day_fable`, …) therefore
//! appear without code changes.

use serde_json::Value;

use crate::time::{now_unix, parse_rfc3339};

pub const SESSION_KEY: &str = "five_hour";
pub const WEEKLY_KEY: &str = "seven_day";
pub const WEEKLY_PREFIX: &str = "seven_day_";
/// Weekly limits that are not tied to a model.
const NON_MODEL_SUFFIXES: &[&str] = &["oauth_apps", "cowork"];
/// Preferred order for the automatic per-model limit.
pub const MODEL_PREFERENCE: &[&str] = &["fable", "opus", "sonnet"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClaudeStatus {
    #[default]
    Disabled,
    Loading,
    Ok,
    NoCredentials,
    Expired,
    Unauthorized,
    RateLimited,
    Error,
}

impl ClaudeStatus {
    /// i18n key describing the status.
    pub fn key(self) -> &'static str {
        match self {
            ClaudeStatus::Disabled => "claude.disabled",
            ClaudeStatus::Loading => "claude.loading",
            ClaudeStatus::Ok => "claude.ok",
            ClaudeStatus::NoCredentials => "claude.no_credentials",
            ClaudeStatus::Expired => "claude.expired",
            ClaudeStatus::Unauthorized => "claude.unauthorized",
            ClaudeStatus::RateLimited => "claude.rate_limited",
            ClaudeStatus::Error => "claude.error",
        }
    }
}

/// One rate-limit window.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeLimit {
    /// Raw key: `five_hour`, `seven_day`, `seven_day_opus`, `seven_day_fable`, …
    pub key: String,
    /// Percent used (0..100, may exceed 100).
    pub utilization: f64,
    /// Unix seconds.
    pub resets_at: Option<i64>,
}

impl ClaudeLimit {
    pub fn is_model_limit(&self) -> bool {
        self.key
            .strip_prefix(WEEKLY_PREFIX)
            .is_some_and(|suffix| !NON_MODEL_SUFFIXES.iter().any(|s| s.eq_ignore_ascii_case(suffix)))
    }

    /// "Opus" for `seven_day_opus`, "Five Hour" for `five_hour`.
    pub fn display_name(&self) -> String {
        let name = self.key.strip_prefix(WEEKLY_PREFIX).unwrap_or(&self.key);
        name.split('_')
            .filter(|w| !w.is_empty())
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn seconds_to_reset(&self, now: i64) -> Option<i64> {
        self.resets_at.map(|r| (r - now).max(0))
    }
}

/// Pay-as-you-go extra usage. Amounts are in cents, as returned by the API.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeExtra {
    pub enabled: bool,
    pub monthly_limit_cents: Option<f64>,
    pub used_cents: Option<f64>,
    pub utilization: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClaudeUsage {
    pub status: ClaudeStatus,
    pub error: Option<String>,
    pub limits: Vec<ClaudeLimit>,
    pub extra: Option<ClaudeExtra>,
    /// Unix seconds of the last successful fetch.
    pub fetched_at: Option<i64>,
    pub subscription: Option<String>,
}

impl ClaudeUsage {
    pub fn with_status(status: ClaudeStatus, error: Option<String>) -> Self {
        Self { status, error, ..Default::default() }
    }

    /// Values are from an earlier successful request; the latest one failed.
    pub fn is_stale(&self) -> bool {
        self.status != ClaudeStatus::Ok && self.fetched_at.is_some()
    }

    pub fn get(&self, key: &str) -> Option<&ClaudeLimit> {
        self.limits.iter().find(|l| l.key.eq_ignore_ascii_case(key))
    }

    pub fn session(&self) -> Option<&ClaudeLimit> {
        self.get(SESSION_KEY)
    }

    pub fn weekly(&self) -> Option<&ClaudeLimit> {
        self.get(WEEKLY_KEY)
    }

    /// Per-model weekly limit. `source` is a model (`fable`), a full key (`seven_day_fable`) or empty/`auto`.
    pub fn model_limit(&self, source: &str) -> Option<&ClaudeLimit> {
        let source = source.trim();
        if !source.is_empty() && !source.eq_ignore_ascii_case("auto") {
            return self.get(source).or_else(|| self.get(&format!("{WEEKLY_PREFIX}{source}")));
        }
        MODEL_PREFERENCE
            .iter()
            .find_map(|m| self.get(&format!("{WEEKLY_PREFIX}{m}")))
            .or_else(|| self.limits.iter().find(|l| l.is_model_limit()))
    }

    /// Keeps the last good values when a refresh fails, so the widget does not flicker to "n/a".
    pub fn merge(previous: &ClaudeUsage, next: ClaudeUsage) -> ClaudeUsage {
        if next.status == ClaudeStatus::Ok || previous.fetched_at.is_none() {
            return next;
        }
        ClaudeUsage {
            status: next.status,
            error: next.error,
            subscription: next.subscription.or_else(|| previous.subscription.clone()),
            ..previous.clone()
        }
    }
}

pub fn parse_usage(json: &str, fetched_at: i64, subscription: Option<String>) -> Result<ClaudeUsage, String> {
    let root: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let obj = root.as_object().ok_or("usage response is not an object")?;

    let mut limits = Vec::new();
    let mut extra = None;
    for (key, value) in obj {
        let Some(entry) = value.as_object() else { continue };
        if key == "extra_usage" {
            extra = Some(ClaudeExtra {
                enabled: entry.get("is_enabled").and_then(Value::as_bool).unwrap_or(false),
                monthly_limit_cents: entry.get("monthly_limit").and_then(number),
                used_cents: entry.get("used_credits").and_then(number),
                utilization: entry.get("utilization").and_then(number),
            });
            continue;
        }
        let Some(utilization) = entry.get("utilization").and_then(number) else { continue };
        let resets_at = entry.get("resets_at").and_then(Value::as_str).and_then(parse_rfc3339);
        limits.push(ClaudeLimit { key: key.clone(), utilization, resets_at });
    }

    Ok(ClaudeUsage { status: ClaudeStatus::Ok, error: None, limits, extra, fetched_at: Some(fetched_at), subscription })
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// OAuth token stored by Claude Code after `/login`. Only ever sent to the usage endpoint.
#[derive(Clone, PartialEq)]
pub struct OAuthToken {
    pub access_token: String,
    /// Unix milliseconds.
    pub expires_at_ms: Option<i64>,
    pub subscription: Option<String>,
}

impl std::fmt::Debug for OAuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthToken")
            .field("access_token", &"<redacted>")
            .field("expires_at_ms", &self.expires_at_ms)
            .field("subscription", &self.subscription)
            .finish()
    }
}

impl OAuthToken {
    pub fn is_expired(&self) -> bool {
        self.expires_at_ms.is_some_and(|ms| ms / 1000 <= now_unix())
    }
}

pub const CREDENTIALS_FILE: &str = ".credentials.json";

pub fn parse_credentials(json: &str) -> Option<OAuthToken> {
    let root: Value = serde_json::from_str(json).ok()?;
    let oauth = root.get("claudeAiOauth")?;
    let token = oauth.get("accessToken")?.as_str()?.to_string();
    if token.is_empty() {
        return None;
    }
    Some(OAuthToken {
        access_token: token,
        expires_at_ms: oauth.get("expiresAt").and_then(Value::as_i64),
        subscription: oauth.get("subscriptionType").and_then(Value::as_str).map(str::to_string),
    })
}

/// Default location of Claude Code credentials, honoring `CLAUDE_CONFIG_DIR`.
pub fn default_credentials_path() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return std::path::PathBuf::from(dir).join(CREDENTIALS_FILE);
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).unwrap_or_default();
    std::path::PathBuf::from(home).join(".claude").join(CREDENTIALS_FILE)
}

/// Accepts a file path or a directory containing `.credentials.json`; empty means default.
pub fn resolve_credentials_path(configured: &str) -> std::path::PathBuf {
    let configured = configured.trim().trim_matches('"');
    if configured.is_empty() {
        return default_credentials_path();
    }
    let path = std::path::PathBuf::from(configured);
    if path.is_dir() { path.join(CREDENTIALS_FILE) } else { path }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "five_hour": {"utilization": 42.0, "resets_at": "2026-10-19T15:00:00.123456+00:00"},
        "seven_day": {"utilization": 17.5, "resets_at": "2026-10-23T09:00:00+00:00"},
        "seven_day_oauth_apps": null,
        "seven_day_opus": {"utilization": 3.0, "resets_at": null},
        "seven_day_fable": {"utilization": 61.0, "resets_at": "2026-10-23T09:00:00Z"},
        "seven_day_sonnet": {"utilization": null, "resets_at": null},
        "extra_usage": {"is_enabled": true, "monthly_limit": 5000, "used_credits": 1250.0, "utilization": 25.0}
    }"#;

    #[test]
    fn parses_all_limits() {
        let u = parse_usage(SAMPLE, 100, Some("max".into())).unwrap();
        assert_eq!(u.status, ClaudeStatus::Ok);
        assert_eq!(u.session().unwrap().utilization, 42.0);
        assert_eq!(u.weekly().unwrap().utilization, 17.5);
        assert_eq!(u.session().unwrap().resets_at, Some(1_792_422_000));
        assert!(u.get("seven_day_sonnet").is_none(), "null utilization is skipped");
        let extra = u.extra.unwrap();
        assert!(extra.enabled);
        assert_eq!(extra.utilization, Some(25.0));
    }

    #[test]
    fn auto_model_prefers_fable() {
        let u = parse_usage(SAMPLE, 0, None).unwrap();
        let m = u.model_limit("auto").unwrap();
        assert_eq!(m.key, "seven_day_fable");
        assert_eq!(m.display_name(), "Fable");
        assert_eq!(u.model_limit("opus").unwrap().utilization, 3.0);
        assert_eq!(u.model_limit("seven_day_opus").unwrap().utilization, 3.0);
        assert!(u.model_limit("haiku").is_none());
    }

    #[test]
    fn unknown_model_limits_are_picked_up() {
        let u = parse_usage(r#"{"seven_day_oauth_apps":{"utilization":5},"seven_day_nova":{"utilization":9}}"#, 0, None).unwrap();
        assert_eq!(u.model_limit("").unwrap().key, "seven_day_nova");
    }

    #[test]
    fn merge_keeps_last_good_values() {
        let good = parse_usage(SAMPLE, 100, None).unwrap();
        let failed = ClaudeUsage::with_status(ClaudeStatus::Error, Some("HTTP 500".into()));
        let merged = ClaudeUsage::merge(&good, failed);
        assert_eq!(merged.status, ClaudeStatus::Error);
        assert!(merged.is_stale());
        assert_eq!(merged.session().unwrap().utilization, 42.0);
    }

    #[test]
    fn parses_credentials() {
        let t = parse_credentials(
            r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-x","refreshToken":"r","expiresAt":1,"subscriptionType":"pro"}}"#,
        )
        .unwrap();
        assert_eq!(t.access_token, "sk-ant-oat01-x");
        assert!(t.is_expired());
        assert_eq!(t.subscription.as_deref(), Some("pro"));
        assert!(!format!("{t:?}").contains("sk-ant"), "token must not leak into logs");
        assert!(parse_credentials("{}").is_none());
    }
}

//! Read-only polling of Codex ChatGPT subscription usage.

use std::time::Duration;

use tm_core::codex::{parse_credentials, parse_usage, resolve_credentials_path};
use tm_core::config::Codex;
use tm_core::time::now_unix;
use tm_core::usage::{SubscriptionUsage, UsageStatus};

use crate::http;

// No configurable URL: Codex OAuth tokens belong only on this trusted HTTPS endpoint.
const USAGE_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";

/// One request. Returns the new state and an optional server-requested back-off.
/// Credentials are never refreshed, written, logged, or included in errors.
pub fn fetch(cfg: &Codex) -> (SubscriptionUsage, Option<Duration>) {
    let path = resolve_credentials_path(&cfg.credentials_path);
    let Ok(text) = std::fs::read_to_string(path) else {
        return (
            SubscriptionUsage::with_status(UsageStatus::NoCredentials, Some("Codex ChatGPT credentials unavailable".into())),
            None,
        );
    };
    let Some(credentials) = parse_credentials(&text) else {
        return (
            SubscriptionUsage::with_status(
                UsageStatus::NoCredentials,
                Some("Codex ChatGPT credentials missing or invalid".into()),
            ),
            None,
        );
    };

    let auth = format!("Bearer {}", credentials.access_token);
    let mut headers = [("Authorization", auth.as_str()), ("Accept", "application/json"), ("ChatGPT-Account-Id", "")];
    let header_count = if let Some(account_id) = credentials.account_id.as_deref() {
        headers[2].1 = account_id;
        3
    } else {
        2
    };
    let user_agent = concat!("taskbar-monitor/", env!("CARGO_PKG_VERSION"));
    let failed = |status, error: String| SubscriptionUsage::with_status(status, Some(error));
    match http::get(USAGE_ENDPOINT, &headers[..header_count], user_agent) {
        Ok(response) if response.status == 200 => match std::str::from_utf8(&response.body) {
            Ok(json) => match parse_usage(json, now_unix()) {
                Ok(usage) => (usage, None),
                Err(error) => (failed(UsageStatus::Error, format!("bad response: {error}")), None),
            },
            Err(_) => (failed(UsageStatus::Error, "invalid usage response encoding".into()), None),
        },
        Ok(response) if response.status == 401 || response.status == 403 => {
            (failed(UsageStatus::Unauthorized, format!("HTTP {}", response.status)), None)
        }
        Ok(response) if response.status == 429 => {
            let wait = Duration::from_secs(response.retry_after.unwrap_or(300).clamp(30, 3600));
            (failed(UsageStatus::RateLimited, "HTTP 429".into()), Some(wait))
        }
        Ok(response) => (failed(UsageStatus::Error, format!("HTTP {}", response.status)), None),
        // Do not expose transport error text that might include request/credential material.
        Err(_) => (failed(UsageStatus::Error, "Codex usage request failed".into()), None),
    }
}

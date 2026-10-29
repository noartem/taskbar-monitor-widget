//! Polls the fixed Ollama Cloud balance endpoint.
//! A nonblank configured API key takes precedence over `OLLAMA_API_KEY`.
//! Both sources are trimmed; credentials and response bodies are never logged or returned as errors.

use std::time::Duration;

use tm_core::config::Ollama;
use tm_core::ollama::parse_usage;
use tm_core::time::now_unix;
use tm_core::usage::{SubscriptionUsage, UsageStatus};

use crate::http;

const BALANCE_ENDPOINT: &str = "https://ollama.com/api/balance";

/// One request, with the same bounded Retry-After back-off as the Claude collector.
pub fn fetch(cfg: &Ollama) -> (SubscriptionUsage, Option<Duration>) {
    let environment_key;
    let key = if cfg.api_key.trim().is_empty() {
        environment_key = std::env::var("OLLAMA_API_KEY").unwrap_or_default();
        environment_key.trim()
    } else {
        cfg.api_key.trim()
    };
    if key.is_empty() {
        return (SubscriptionUsage::with_status(UsageStatus::NoCredentials, None), None);
    }
    if !valid_key(key) {
        return failure(UsageStatus::Unauthorized, "invalid API key");
    }
    let auth = format!("Bearer {key}");
    let headers = [("Authorization", auth.as_str()), ("Accept", "application/json")];
    let user_agent = concat!("taskbar-monitor/", env!("CARGO_PKG_VERSION"));
    match http::get(BALANCE_ENDPOINT, &headers, user_agent) {
        Ok(response) if response.status == 200 => match parse_usage(&String::from_utf8_lossy(&response.body), now_unix()) {
            Ok(usage) => (usage, None),
            Err(_) => failure(UsageStatus::Error, "invalid balance response"),
        },
        Ok(response) if response.status == 401 || response.status == 403 => {
            failure(UsageStatus::Unauthorized, &format!("HTTP {}", response.status))
        }
        Ok(response) if response.status == 429 => (
            SubscriptionUsage::with_status(UsageStatus::RateLimited, Some("HTTP 429".into())),
            Some(Duration::from_secs(response.retry_after.unwrap_or(300).clamp(30, 3600))),
        ),
        Ok(response) => failure(UsageStatus::Error, &format!("HTTP {}", response.status)),
        Err(_) => failure(UsageStatus::Error, "balance request failed"),
    }
}

fn failure(status: UsageStatus, message: &str) -> (SubscriptionUsage, Option<Duration>) {
    (SubscriptionUsage::with_status(status, Some(message.into())), None)
}

// Reject header delimiters and non-ASCII input before constructing Authorization.
fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|b| b.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use super::valid_key;

    #[test]
    fn api_key_cannot_inject_headers() {
        assert!(valid_key("ollama-test_key.123"));
        for key in ["", "key\r\nX-Injected: yes", "key\0suffix", "key with space", "kéy"] {
            assert!(!valid_key(key));
        }
    }
}

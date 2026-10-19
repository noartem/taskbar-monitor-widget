//! Background polling of Claude Code subscription usage.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tm_core::claude::{ClaudeStatus, ClaudeUsage, parse_credentials, parse_usage, resolve_credentials_path};
use tm_core::config::Claude;
use tm_core::time::now_unix;

use super::Shared;
use crate::http;

/// One request. Returns the new state and an optional server-requested back-off.
pub fn fetch(cfg: &Claude) -> (ClaudeUsage, Option<Duration>) {
    let path = resolve_credentials_path(&cfg.credentials_path);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (ClaudeUsage::with_status(ClaudeStatus::NoCredentials, Some(path.display().to_string())), None);
    };
    let Some(token) = parse_credentials(&text) else {
        return (ClaudeUsage::with_status(ClaudeStatus::NoCredentials, Some(path.display().to_string())), None);
    };
    let subscription = token.subscription.clone();
    let with = |status, error: String| ClaudeUsage {
        subscription: subscription.clone(),
        ..ClaudeUsage::with_status(status, Some(error))
    };
    if token.is_expired() {
        return (with(ClaudeStatus::Expired, "token expired".into()), None);
    }

    let auth = format!("Bearer {}", token.access_token);
    let user_agent = concat!("taskbar-monitor/", env!("CARGO_PKG_VERSION"));
    let headers = [("Authorization", auth.as_str()), ("Accept", "application/json"), ("anthropic-beta", "oauth-2025-04-20")];
    match http::get(&cfg.usage_endpoint, &headers, user_agent) {
        Ok(r) if r.status == 200 => match parse_usage(&String::from_utf8_lossy(&r.body), now_unix(), subscription.clone()) {
            Ok(u) => (u, None),
            Err(e) => (with(ClaudeStatus::Error, format!("bad response: {e}")), None),
        },
        Ok(r) if r.status == 401 || r.status == 403 => (with(ClaudeStatus::Unauthorized, format!("HTTP {}", r.status)), None),
        Ok(r) if r.status == 429 => {
            let wait = Duration::from_secs(r.retry_after.unwrap_or(300).clamp(30, 3600));
            (with(ClaudeStatus::RateLimited, "HTTP 429".into()), Some(wait))
        }
        Ok(r) => (with(ClaudeStatus::Error, format!("HTTP {}", r.status)), None),
        Err(e) => (with(ClaudeStatus::Error, e), None),
    }
}

pub fn run(shared: Arc<Shared>) {
    loop {
        let (cfg, enabled) = {
            let st = shared.state.lock().unwrap();
            if st.stop {
                return;
            }
            let enabled = st.req.claude.enabled && st.req.sources.contains(tm_core::metrics::DataSources::CLAUDE);
            (st.req.claude.clone(), enabled)
        };

        let mut wait = Duration::from_secs(cfg.poll_interval_secs as u64);
        if enabled {
            {
                let mut st = shared.state.lock().unwrap();
                if st.claude.status == ClaudeStatus::Disabled {
                    st.claude = ClaudeUsage::with_status(ClaudeStatus::Loading, None);
                }
            }
            let (result, backoff) = fetch(&cfg);
            if result.status != ClaudeStatus::Ok {
                crate::log!("claude usage: {:?} {:?}", result.status, result.error);
            }
            if let Some(b) = backoff {
                wait = wait.max(b);
            }
            let mut st = shared.state.lock().unwrap();
            st.claude = ClaudeUsage::merge(&st.claude, result);
        } else {
            shared.state.lock().unwrap().claude = ClaudeUsage::default();
        }
        shared.notify_ui();

        // Sleep until the interval passes, settings change or a manual refresh is requested.
        let deadline = Instant::now() + wait;
        let mut st = shared.state.lock().unwrap();
        let generation = st.claude_generation;
        loop {
            if st.stop || st.claude_refresh || st.claude_generation != generation {
                st.claude_refresh = false;
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            st = shared.claude_wake.wait_timeout(st, deadline - now).unwrap().0;
        }
    }
}

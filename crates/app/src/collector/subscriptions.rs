//! Independent quota pollers; generations prevent publishing a response for obsolete credentials.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tm_core::metrics::DataSources;
use tm_core::usage::{SubscriptionUsage, UsageStatus};

use super::{Shared, SharedState};

enum Request {
    Codex(tm_core::config::Codex),
    Ollama(tm_core::config::Ollama),
}

fn usage(st: &mut SharedState, provider: usize) -> &mut SubscriptionUsage {
    if provider == 0 { &mut st.codex } else { &mut st.ollama }
}

pub fn run(shared: Arc<Shared>, provider: usize) {
    loop {
        let (request, enabled, generation) = {
            let mut st = shared.state.lock().unwrap();
            if st.stop {
                return;
            }
            st.subscription_refresh[provider] = false;
            let enabled = if provider == 0 {
                st.req.codex.enabled && st.req.sources.contains(DataSources::CODEX)
            } else {
                st.req.ollama.enabled && st.req.sources.contains(DataSources::OLLAMA)
            };
            let current = usage(&mut st, provider);
            if !enabled {
                *current = SubscriptionUsage::default();
            } else if current.status == UsageStatus::Disabled {
                *current = SubscriptionUsage::with_status(UsageStatus::Loading, None);
            }
            let request =
                if provider == 0 { Request::Codex(st.req.codex.clone()) } else { Request::Ollama(st.req.ollama.clone()) };
            (request, enabled, st.subscription_generations[provider])
        };
        shared.notify_ui();
        let interval = match &request {
            Request::Codex(cfg) => cfg.poll_interval_secs,
            Request::Ollama(cfg) => cfg.poll_interval_secs,
        };
        let mut wait = Duration::from_secs(interval as u64);
        if enabled {
            let (result, backoff) = match request {
                Request::Codex(cfg) => super::codex::fetch(&cfg),
                Request::Ollama(cfg) => super::ollama::fetch(&cfg),
            };
            if let Some(backoff) = backoff {
                wait = wait.max(backoff);
            }
            let mut st = shared.state.lock().unwrap();
            if st.stop {
                return;
            }
            if st.subscription_generations[provider] != generation {
                continue;
            }
            let current = usage(&mut st, provider);
            *current = SubscriptionUsage::merge(current, result);
            drop(st);
            shared.notify_ui();
        }
        let deadline = Instant::now() + wait;
        let mut st = shared.state.lock().unwrap();
        while !st.stop && !st.subscription_refresh[provider] && st.subscription_generations[provider] == generation {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            st = shared.subscription_wake.wait_timeout(st, deadline - now).unwrap().0;
        }
    }
}

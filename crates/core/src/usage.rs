//! Shared state for subscription quota windows (all percentages are consumed, not remaining).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UsageStatus {
    #[default]
    Disabled,
    Loading,
    Ok,
    NoCredentials,
    Unauthorized,
    RateLimited,
    Error,
}

impl UsageStatus {
    pub fn key(self) -> &'static str {
        match self {
            Self::Disabled => "usage.disabled",
            Self::Loading => "usage.loading",
            Self::Ok => "usage.ok",
            Self::NoCredentials => "usage.no_credentials",
            Self::Unauthorized => "usage.unauthorized",
            Self::RateLimited => "usage.rate_limited",
            Self::Error => "usage.error",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageLimit {
    pub key: &'static str,
    pub utilization: f64,
    pub resets_at: Option<i64>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SubscriptionUsage {
    pub status: UsageStatus,
    pub error: Option<String>,
    pub limits: Vec<UsageLimit>,
    pub fetched_at: Option<i64>,
    pub subscription: Option<String>,
}

impl SubscriptionUsage {
    pub fn with_status(status: UsageStatus, error: Option<String>) -> Self {
        Self { status, error, ..Default::default() }
    }

    pub fn get(&self, key: &str) -> Option<&UsageLimit> {
        self.limits.iter().find(|l| l.key == key)
    }

    pub fn is_stale(&self) -> bool {
        self.status != UsageStatus::Ok && self.fetched_at.is_some()
    }

    pub fn merge(previous: &Self, next: Self) -> Self {
        if next.status == UsageStatus::Ok || previous.fetched_at.is_none() {
            return next;
        }
        Self { status: next.status, error: next.error, ..previous.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_refresh_keeps_values_until_a_success_replaces_them() {
        let good = SubscriptionUsage {
            status: UsageStatus::Ok,
            limits: vec![UsageLimit { key: "weekly", utilization: 42.0, resets_at: Some(200), detail: None }],
            fetched_at: Some(100),
            ..Default::default()
        };
        let failed = SubscriptionUsage::with_status(UsageStatus::Unauthorized, None);
        let stale = SubscriptionUsage::merge(&good, failed.clone());
        assert!(stale.is_stale());
        assert_eq!(stale.status, UsageStatus::Unauthorized);
        assert_eq!(stale.get("weekly").unwrap().utilization, 42.0);
        assert_eq!(stale.fetched_at, Some(100));
        let empty_success = SubscriptionUsage { status: UsageStatus::Ok, fetched_at: Some(150), ..Default::default() };
        let recovered = SubscriptionUsage::merge(&stale, empty_success);
        assert!(!recovered.is_stale());
        assert!(recovered.get("weekly").is_none());
        assert_eq!(SubscriptionUsage::merge(&SubscriptionUsage::default(), failed).fetched_at, None);
    }
}

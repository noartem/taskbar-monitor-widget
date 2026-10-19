//! Ready-made layouts, selectable from the tray menu and the settings window.

use crate::config::{MetricConfig, Settings};
use crate::metrics::{MetricKind as K, MetricStyle as S};

pub struct Preset {
    /// i18n key suffix: `preset.<key>`.
    pub key: &'static str,
    pub rows: u32,
    pub metrics: fn() -> Vec<MetricConfig>,
}

pub const ALL: &[Preset] = &[
    Preset { key: "graphs", rows: 2, metrics: default_metrics },
    Preset { key: "compact", rows: 1, metrics: compact },
    Preset { key: "minimal", rows: 2, metrics: minimal },
    Preset { key: "rings", rows: 1, metrics: rings },
    Preset { key: "claude", rows: 2, metrics: claude },
    Preset { key: "dense", rows: 3, metrics: dense },
];

pub fn find(key: &str) -> Option<&'static Preset> {
    ALL.iter().find(|p| p.key.eq_ignore_ascii_case(key))
}

pub fn apply(settings: &mut Settings, preset: &Preset) {
    settings.layout.rows = preset.rows;
    settings.metrics = (preset.metrics)();
}

fn m(kind: K, style: S) -> MetricConfig {
    MetricConfig::styled(kind, style)
}

/// Shown only in the details flyout by most presets.
fn flyout_extras() -> Vec<MetricConfig> {
    vec![
        m(K::CpuTemp, S::Text).flyout_only(),
        m(K::GpuTemp, S::Text).flyout_only(),
        m(K::DiskActive, S::Line).flyout_only(),
        m(K::DiskRead, S::Text).flyout_only(),
        m(K::DiskWrite, S::Text).flyout_only(),
        m(K::DiskSpace, S::Bar).flyout_only().with_source("C:"),
        m(K::ClaudeExtra, S::Bar).flyout_only(),
    ]
}

/// Line graphs for load, bars for memory and Claude limits, text for network.
pub fn default_metrics() -> Vec<MetricConfig> {
    let mut v = vec![
        m(K::CpuUsage, S::Line),
        m(K::GpuUsage, S::Line),
        m(K::RamUsage, S::Bar),
        m(K::VramUsage, S::Bar),
        m(K::NetDown, S::Text),
        m(K::NetUp, S::Text),
        m(K::ClaudeSession, S::Bar),
        m(K::ClaudeWeekly, S::Bar),
        m(K::ClaudeModel, S::Bar),
    ];
    v.extend(flyout_extras());
    v
}

fn compact() -> Vec<MetricConfig> {
    let mut v = vec![
        m(K::CpuUsage, S::Text),
        m(K::RamUsage, S::Text),
        m(K::GpuUsage, S::Text),
        m(K::NetDown, S::Text),
        m(K::ClaudeSession, S::Text),
        m(K::ClaudeWeekly, S::Text),
        m(K::VramUsage, S::Text).flyout_only(),
        m(K::NetUp, S::Text).flyout_only(),
        m(K::ClaudeModel, S::Text).flyout_only(),
    ];
    v.extend(flyout_extras());
    v
}

fn minimal() -> Vec<MetricConfig> {
    let mut v = vec![
        m(K::CpuUsage, S::Fill),
        m(K::RamUsage, S::Fill),
        m(K::GpuUsage, S::Fill),
        m(K::VramUsage, S::Fill),
        m(K::ClaudeSession, S::Fill),
        m(K::ClaudeWeekly, S::Fill),
        m(K::NetDown, S::Text).flyout_only(),
        m(K::NetUp, S::Text).flyout_only(),
        m(K::ClaudeModel, S::Fill).flyout_only(),
    ];
    v.extend(flyout_extras());
    v
}

fn rings() -> Vec<MetricConfig> {
    let mut v = vec![
        m(K::CpuUsage, S::Ring),
        m(K::RamUsage, S::Ring),
        m(K::GpuUsage, S::Ring),
        m(K::VramUsage, S::Ring),
        m(K::ClaudeSession, S::Ring),
        m(K::ClaudeWeekly, S::Ring),
        m(K::ClaudeModel, S::Ring),
        m(K::NetDown, S::Text).flyout_only(),
        m(K::NetUp, S::Text).flyout_only(),
    ];
    v.extend(flyout_extras());
    v
}

fn claude() -> Vec<MetricConfig> {
    vec![
        m(K::ClaudeSession, S::Bar),
        m(K::ClaudeWeekly, S::Bar),
        m(K::ClaudeModel, S::Bar),
        m(K::ClaudeExtra, S::Bar),
        m(K::CpuUsage, S::Line).flyout_only(),
        m(K::RamUsage, S::Bar).flyout_only(),
        m(K::GpuUsage, S::Line).flyout_only(),
        m(K::VramUsage, S::Bar).flyout_only(),
    ]
}

fn dense() -> Vec<MetricConfig> {
    vec![
        m(K::CpuUsage, S::Text),
        m(K::CpuTemp, S::Text),
        m(K::RamUsage, S::Text),
        m(K::GpuUsage, S::Text),
        m(K::GpuTemp, S::Text),
        m(K::VramUsage, S::Text),
        m(K::NetDown, S::Text),
        m(K::NetUp, S::Text),
        m(K::DiskActive, S::Text),
        m(K::ClaudeSession, S::Text),
        m(K::ClaudeWeekly, S::Text),
        m(K::ClaudeModel, S::Text),
        m(K::DiskRead, S::Text).flyout_only(),
        m(K::DiskWrite, S::Text).flyout_only(),
        m(K::DiskSpace, S::Bar).flyout_only().with_source("C:"),
    ]
}

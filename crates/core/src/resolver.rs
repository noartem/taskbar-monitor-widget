//! Evaluates a [`MetricConfig`] against a [`Snapshot`].

use crate::claude::{ClaudeLimit, ClaudeStatus, ClaudeUsage};
use crate::config::{GpuDisplay, Hardware, MetricConfig, NetworkUnit, Settings};
use crate::format;
use crate::metrics::{MetricKind, SourceType};
use crate::snapshot::{DiskInfo, GpuInfo, Snapshot};
use crate::time::now_unix;

#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub available: bool,
    /// Raw value in the metric's unit; `NaN` when unavailable.
    pub value: f64,
    /// 0..1 position for bars and rings; `None` when the scale comes from history (rates).
    pub fraction: Option<f64>,
    pub value_text: String,
    pub label: String,
    /// Longer description, e.g. `12.3 GB / 32.0 GB` or a reset countdown.
    pub detail: Option<String>,
    /// Device the value came from (GPU, adapter, disk…).
    pub source_name: Option<String>,
    /// Seconds until a Claude limit resets.
    pub resets_in: Option<i64>,
    /// i18n key explaining why data is missing or stale.
    pub status: Option<&'static str>,
    /// Whether warning/critical colors apply.
    pub uses_thresholds: bool,
}

impl Reading {
    pub fn unavailable(label: String, status: Option<&'static str>) -> Self {
        Self {
            available: false,
            value: f64::NAN,
            fraction: None,
            value_text: "—".into(),
            label,
            detail: None,
            source_name: None,
            resets_in: None,
            status,
            uses_thresholds: false,
        }
    }

    fn percent(label: String, value: f64, decimals: i32, source: Option<String>) -> Self {
        Self {
            available: true,
            value,
            fraction: Some((value / 100.0).clamp(0.0, 1.0)),
            value_text: format::percent(value, decimals),
            label,
            detail: None,
            source_name: source,
            resets_in: None,
            status: None,
            uses_thresholds: true,
        }
    }

    fn temperature(label: String, value: f64, decimals: i32, source: Option<String>) -> Self {
        Self { value_text: format::temperature(value, decimals), ..Self::percent(label, value, decimals, source) }
    }

    fn rate(label: String, value: f64, unit: NetworkUnit, decimals: i32, max: f64, source: Option<String>) -> Self {
        Self {
            available: true,
            value,
            fraction: (max > 0.0).then(|| (value / max).clamp(0.0, 1.0)),
            value_text: format::rate(value, unit, decimals),
            label,
            detail: None,
            source_name: source,
            resets_in: None,
            status: None,
            uses_thresholds: false,
        }
    }

    fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }
}

pub fn label(config: &MetricConfig, snap: &Snapshot) -> String {
    if !config.label.is_empty() {
        return config.label.clone();
    }
    match config.kind {
        MetricKind::ClaudeModel => {
            snap.claude.model_limit(&config.source).map(ClaudeLimit::display_name).unwrap_or_else(|| match config.source.trim() {
                "" | "auto" => "Model".into(),
                s => capitalize(s),
            })
        }
        MetricKind::DiskSpace => normalize_drive(&config.source),
        MetricKind::Sensor => {
            snap.sensors.get(&config.source).map(|s| s.name.chars().take(10).collect()).unwrap_or_else(|| "Sensor".into())
        }
        kind => kind.def().default_label.into(),
    }
}

pub fn resolve(config: &MetricConfig, snap: &Snapshot, settings: &Settings) -> Reading {
    let label = label(config, snap);
    let dec = config.decimals;
    let hw = &settings.hardware;

    match config.kind {
        MetricKind::CpuUsage => match snap.cpu_usage {
            Some(v) => Reading::percent(label, v, dec, snap.cpu_name.clone()),
            None => Reading::unavailable(label, None),
        },
        MetricKind::CpuTemp => match snap.cpu_temperature {
            Some(v) => Reading::temperature(label, v, dec, snap.cpu_name.clone()),
            None => Reading::unavailable(label, Some("status.no_sensor")),
        },
        MetricKind::RamUsage => {
            if snap.ram_total == 0 {
                return Reading::unavailable(label, None);
            }
            Reading::percent(label, 100.0 * snap.ram_used as f64 / snap.ram_total as f64, dec, None).with_detail(format!(
                "{} / {}",
                format::bytes(snap.ram_used as f64, -1),
                format::bytes(snap.ram_total as f64, -1)
            ))
        }
        MetricKind::GpuUsage => match select_gpu(snap, &config.source, hw) {
            Some(GpuInfo { usage: Some(u), name, .. }) => Reading::percent(label, *u, dec, Some(name.clone())),
            _ => Reading::unavailable(label, None),
        },
        MetricKind::GpuTemp => match select_gpu(snap, &config.source, hw) {
            Some(GpuInfo { temperature: Some(t), name, fan_rpm, .. }) => {
                let r = Reading::temperature(label, *t, dec, Some(name.clone()));
                match fan_rpm {
                    Some(rpm) if *rpm > 0 => r.with_detail(format!("{rpm} RPM")),
                    _ => r,
                }
            }
            _ => Reading::unavailable(label, Some("status.no_sensor")),
        },
        MetricKind::VramUsage => match select_gpu(snap, &config.source, hw) {
            Some(g) if g.vram_total > 0 => {
                Reading::percent(label, 100.0 * g.vram_used as f64 / g.vram_total as f64, dec, Some(g.name.clone())).with_detail(
                    format!("{} / {}", format::bytes(g.vram_used as f64, -1), format::bytes(g.vram_total as f64, -1)),
                )
            }
            _ => Reading::unavailable(label, None),
        },
        MetricKind::NetDown | MetricKind::NetUp | MetricKind::NetTotal => {
            let Some((down, up, name)) = sum_networks(snap, &config.source) else {
                return Reading::unavailable(label, None);
            };
            let value = match config.kind {
                MetricKind::NetDown => down,
                MetricKind::NetUp => up,
                _ => down + up,
            };
            let unit = settings.general.network_unit;
            Reading::rate(label, value, unit, dec, config.max_value, Some(name)).with_detail(format!(
                "↓ {}   ↑ {}",
                format::rate(down, unit, -1),
                format::rate(up, unit, -1)
            ))
        }
        MetricKind::DiskRead | MetricKind::DiskWrite | MetricKind::DiskActive => {
            let Some(disk) = select_disk(snap, &config.source) else {
                return Reading::unavailable(label, None);
            };
            let src = Some(disk.name.clone());
            let r = match config.kind {
                MetricKind::DiskRead => Reading::rate(label, disk.read, NetworkUnit::Bytes, dec, config.max_value, src),
                MetricKind::DiskWrite => Reading::rate(label, disk.write, NetworkUnit::Bytes, dec, config.max_value, src),
                _ => Reading::percent(label, disk.active, dec, src),
            };
            r.with_detail(format!(
                "R {}   W {}",
                format::rate(disk.read, NetworkUnit::Bytes, -1),
                format::rate(disk.write, NetworkUnit::Bytes, -1)
            ))
        }
        MetricKind::DiskSpace => {
            let name = normalize_drive(&config.source);
            match snap.drives.iter().find(|d| d.name.eq_ignore_ascii_case(&name)) {
                Some(d) if d.total > 0 => {
                    Reading::percent(label, 100.0 * d.used as f64 / d.total as f64, dec, Some(d.name.clone()))
                        .with_detail(format!("{} / {}", format::bytes(d.used as f64, -1), format::bytes(d.total as f64, -1)))
                }
                _ => Reading::unavailable(label, None),
            }
        }
        MetricKind::ClaudeSession => claude_reading(label, &snap.claude, snap.claude.session(), dec, settings),
        MetricKind::ClaudeWeekly => claude_reading(label, &snap.claude, snap.claude.weekly(), dec, settings),
        MetricKind::ClaudeModel => claude_reading(label, &snap.claude, snap.claude.model_limit(&config.source), dec, settings),
        MetricKind::ClaudeExtra => match &snap.claude.extra {
            Some(e) if e.enabled && e.utilization.is_some() => {
                let mut r = Reading::percent(label, e.utilization.unwrap_or_default(), dec, None);
                if let (Some(used), Some(limit)) = (e.used_cents, e.monthly_limit_cents) {
                    r.detail = Some(format!("${:.2} / ${:.2}", used / 100.0, limit / 100.0));
                }
                r.status = snap.claude.is_stale().then_some("claude.stale");
                r
            }
            _ => Reading::unavailable(label, Some(claude_status_key(&snap.claude))),
        },
        MetricKind::Sensor => match snap.sensors.get(&config.source) {
            Some(s) => {
                let fraction = if s.unit == "%" {
                    Some((s.value / 100.0).clamp(0.0, 1.0))
                } else if config.max_value > 0.0 {
                    Some((s.value / config.max_value).clamp(0.0, 1.0))
                } else {
                    None
                };
                Reading {
                    available: true,
                    value: s.value,
                    fraction,
                    value_text: format::generic(s.value, &s.unit, dec),
                    label,
                    detail: Some(format!("{} · {}", s.hardware, s.name)),
                    source_name: Some(s.hardware.clone()),
                    resets_in: None,
                    status: None,
                    uses_thresholds: s.unit == "%" || s.unit == "°C",
                }
            }
            None => Reading::unavailable(label, Some("status.no_sensor")),
        },
    }
}

fn claude_status_key(usage: &ClaudeUsage) -> &'static str {
    if usage.status == ClaudeStatus::Ok { "claude.not_available" } else { usage.status.key() }
}

fn claude_reading(label: String, usage: &ClaudeUsage, limit: Option<&ClaudeLimit>, dec: i32, settings: &Settings) -> Reading {
    let Some(limit) = limit else {
        return Reading::unavailable(label, Some(claude_status_key(usage)));
    };
    let mut r = Reading::percent(label, limit.utilization, dec, Some(limit.display_name()));
    r.resets_in = limit.seconds_to_reset(now_unix());
    if settings.claude.show_reset_countdown {
        r.detail = r.resets_in.map(|s| format!("↻ {}", format::duration(s)));
    }
    r.status = usage.is_stale().then_some("claude.stale");
    r
}

/// GPU for a metric. `source`: GPU name substring or LUID, `*` = busiest; empty = per `hw.gpu_display`
/// (`all` is expanded into explicit sources by [`expand_metrics`] beforehand).
pub fn select_gpu<'a>(snap: &'a Snapshot, source: &str, hw: &Hardware) -> Option<&'a GpuInfo> {
    let busiest = || {
        snap.gpus
            .iter()
            .max_by(|a, b| a.usage.unwrap_or(0.0).total_cmp(&b.usage.unwrap_or(0.0)).then(a.vram_used.cmp(&b.vram_used)))
    };
    let main =
        || find_gpu(snap, &hw.gpu_adapter).or_else(|| snap.gpus.iter().find(|g| g.is_default)).or_else(|| snap.gpus.first());
    match source.trim() {
        "*" => busiest(),
        "" if hw.gpu_display == GpuDisplay::Busiest => busiest(),
        "" => main(),
        selector => find_gpu(snap, selector).or_else(main),
    }
}

fn find_gpu<'a>(snap: &'a Snapshot, selector: &str) -> Option<&'a GpuInfo> {
    let selector = selector.trim();
    if selector.is_empty() {
        return None;
    }
    let needle = selector.to_lowercase();
    snap.gpus
        .iter()
        .find(|g| g.luid.eq_ignore_ascii_case(selector))
        .or_else(|| snap.gpus.iter().find(|g| g.name.to_lowercase().contains(&needle)))
}

/// With `gpu_display = all`, every GPU metric without an explicit source becomes one tile per GPU
/// (labelled `GPU1`, `VRAM2`…, most video memory first). Other metrics are returned unchanged.
pub fn expand_metrics(metrics: &[MetricConfig], snap: &Snapshot, hw: &Hardware) -> Vec<MetricConfig> {
    let mut gpus: Vec<&GpuInfo> = snap.gpus.iter().filter(|g| g.listed).collect();
    gpus.sort_by_key(|g| std::cmp::Reverse(g.vram_total));
    if hw.gpu_display != GpuDisplay::All || gpus.len() < 2 {
        return metrics.to_vec();
    }
    let mut out = Vec::with_capacity(metrics.len() + gpus.len());
    for m in metrics {
        if m.kind.def().source_type != SourceType::Gpu || !m.source.trim().is_empty() {
            out.push(m.clone());
            continue;
        }
        let base = if m.label.is_empty() { m.kind.def().default_label.to_string() } else { m.label.clone() };
        for (i, g) in gpus.iter().enumerate() {
            out.push(MetricConfig { source: g.luid.clone(), label: format!("{base}{}", i + 1), ..m.clone() });
        }
    }
    out
}

/// Empty source = physical adapters (avoids double counting virtual switches and VPNs), `*` = all,
/// otherwise a name/description substring. Returns `(down, up, adapter names)`.
pub fn sum_networks(snap: &Snapshot, source: &str) -> Option<(f64, f64, String)> {
    if snap.networks.is_empty() {
        return None;
    }
    let source = source.trim();
    let needle = source.to_lowercase();
    let selected: Vec<_> = match source {
        "" => {
            let hw: Vec<_> = snap.networks.iter().filter(|n| n.is_hardware).collect();
            if hw.is_empty() { snap.networks.iter().collect() } else { hw }
        }
        "*" => snap.networks.iter().collect(),
        _ => snap
            .networks
            .iter()
            .filter(|n| n.name.to_lowercase().contains(&needle) || n.description.to_lowercase().contains(&needle))
            .collect(),
    };
    if selected.is_empty() {
        return None;
    }
    let down = selected.iter().map(|n| n.down).sum();
    let up = selected.iter().map(|n| n.up).sum();
    let names = selected.iter().map(|n| n.name.as_str()).collect::<Vec<_>>().join(", ");
    Some((down, up, names))
}

pub fn select_disk<'a>(snap: &'a Snapshot, source: &str) -> Option<&'a DiskInfo> {
    let source = source.trim();
    if source.is_empty() {
        return snap.disks.iter().find(|d| d.name == "_Total").or_else(|| snap.disks.first());
    }
    let needle = source.to_lowercase();
    snap.disks
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case(source))
        .or_else(|| snap.disks.iter().find(|d| d.name != "_Total" && d.name.to_lowercase().contains(&needle)))
}

pub fn normalize_drive(source: &str) -> String {
    let s = source.trim().trim_end_matches(['\\', '/']);
    if s.is_empty() {
        return "C:".into();
    }
    let mut s = s.to_uppercase();
    if s.len() == 1 && s.chars().all(|c| c.is_ascii_alphabetic()) {
        s.push(':');
    }
    s
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::parse_usage;
    use crate::snapshot::NetworkInfo;

    fn snap() -> Snapshot {
        Snapshot {
            cpu_usage: Some(12.6),
            ram_used: 8 << 30,
            ram_total: 32 << 30,
            gpus: vec![
                GpuInfo {
                    name: "Intel UHD".into(),
                    luid: "a".into(),
                    usage: Some(1.0),
                    vram_total: 128 << 20,
                    listed: true,
                    ..Default::default()
                },
                GpuInfo {
                    name: "NVIDIA RTX".into(),
                    luid: "b".into(),
                    usage: Some(50.0),
                    vram_used: 4 << 30,
                    vram_total: 16 << 30,
                    is_default: true,
                    listed: true,
                    ..Default::default()
                },
            ],
            networks: vec![
                NetworkInfo { name: "Ethernet".into(), down: 1000.0, up: 10.0, is_hardware: true, ..Default::default() },
                NetworkInfo { name: "vEthernet".into(), down: 1000.0, up: 10.0, is_hardware: false, ..Default::default() },
            ],
            claude: parse_usage(r#"{"five_hour":{"utilization":33},"seven_day_fable":{"utilization":7}}"#, 0, None).unwrap(),
            ..Default::default()
        }
    }

    #[test]
    fn resolves_basic_metrics() {
        let s = Settings::default();
        let sn = snap();
        let cpu = resolve(&MetricConfig::new(MetricKind::CpuUsage), &sn, &s);
        assert_eq!(cpu.value_text, "13%");
        assert_eq!(cpu.label, "CPU");
        let ram = resolve(&MetricConfig::new(MetricKind::RamUsage), &sn, &s);
        assert_eq!(ram.value_text, "25%");
        assert_eq!(ram.detail.as_deref(), Some("8.0 GB / 32.0 GB"));
    }

    #[test]
    fn gpu_selection() {
        let s = Settings::default();
        let sn = snap();
        let vram = resolve(&MetricConfig::new(MetricKind::VramUsage), &sn, &s);
        assert_eq!(vram.value_text, "25%");
        let intel = resolve(&MetricConfig::new(MetricKind::GpuUsage).with_source("intel"), &sn, &s);
        assert_eq!(intel.value, 1.0);
    }

    #[test]
    fn busiest_gpu_selector() {
        let s = Settings::default();
        let mut sn = snap();
        assert_eq!(resolve(&MetricConfig::new(MetricKind::GpuUsage).with_source("*"), &sn, &s).value, 50.0);
        sn.gpus[0].usage = Some(80.0);
        assert_eq!(resolve(&MetricConfig::new(MetricKind::GpuUsage).with_source("*"), &sn, &s).value, 80.0);
        assert_eq!(resolve(&MetricConfig::new(MetricKind::GpuUsage), &sn, &s).value, 50.0, "default stays on the default GPU");
        let busiest =
            Settings { hardware: Hardware { gpu_display: GpuDisplay::Busiest, ..Hardware::default() }, ..Settings::default() };
        assert_eq!(resolve(&MetricConfig::new(MetricKind::GpuUsage), &sn, &busiest).value, 80.0);
    }

    #[test]
    fn all_gpus_expand_into_one_tile_each() {
        let sn = snap();
        let hw = Hardware { gpu_display: GpuDisplay::All, ..Hardware::default() };
        let metrics = vec![
            MetricConfig::new(MetricKind::CpuUsage),
            MetricConfig::new(MetricKind::GpuUsage),
            MetricConfig::new(MetricKind::VramUsage).with_source("intel"),
        ];
        let out = expand_metrics(&metrics, &sn, &hw);
        let labels: Vec<_> = out.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["", "GPU1", "GPU2", ""]);
        assert_eq!(out[1].source, "b", "most video memory first");
        let s = Settings { hardware: hw, ..Settings::default() };
        assert_eq!(resolve(&out[1], &sn, &s).value, 50.0);
        assert_eq!(resolve(&out[2], &sn, &s).value, 1.0);
        assert_eq!(expand_metrics(&metrics, &sn, &Hardware::default()).len(), 3, "main mode leaves metrics alone");
    }

    #[test]
    fn network_defaults_to_hardware_adapters() {
        let s = Settings::default();
        let sn = snap();
        assert_eq!(resolve(&MetricConfig::new(MetricKind::NetDown), &sn, &s).value, 1000.0);
        assert_eq!(resolve(&MetricConfig::new(MetricKind::NetDown).with_source("*"), &sn, &s).value, 2000.0);
        assert!(!resolve(&MetricConfig::new(MetricKind::NetDown).with_source("wifi"), &sn, &s).available);
    }

    #[test]
    fn claude_metrics() {
        let s = Settings::default();
        let sn = snap();
        let session = resolve(&MetricConfig::new(MetricKind::ClaudeSession), &sn, &s);
        assert_eq!(session.value_text, "33%");
        let model = resolve(&MetricConfig::new(MetricKind::ClaudeModel), &sn, &s);
        assert_eq!(model.label, "Fable");
        assert_eq!(model.value, 7.0);
        let weekly = resolve(&MetricConfig::new(MetricKind::ClaudeWeekly), &sn, &s);
        assert!(!weekly.available);
        assert_eq!(weekly.status, Some("claude.not_available"));
    }

    #[test]
    fn drive_normalization() {
        assert_eq!(normalize_drive("d"), "D:");
        assert_eq!(normalize_drive("e:\\"), "E:");
        assert_eq!(normalize_drive(""), "C:");
    }
}

//! `config.toml`: every field has a default, so a partial or hand-edited file is valid.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::metrics::{MetricKind, MetricStyle};
use crate::presets;

pub const CONFIG_FILE: &str = "config.toml";
/// Marker file shipped in the portable zip, next to the executable.
pub const PORTABLE_MARKER: &str = "portable.txt";
pub const APP_DIR: &str = "TaskbarMonitor";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub general: General,
    pub appearance: Appearance,
    pub layout: Layout,
    pub tray: Tray,
    pub claude: Claude,
    pub codex: Codex,
    pub ollama: Ollama,
    pub hardware: Hardware,
    pub metrics: Vec<MetricConfig>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            general: General::default(),
            appearance: Appearance::default(),
            layout: Layout::default(),
            tray: Tray::default(),
            claude: Claude::default(),
            codex: Codex::default(),
            ollama: Ollama::default(),
            hardware: Hardware::default(),
            metrics: presets::default_metrics(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Autostart {
    #[default]
    Off,
    /// HKCU Run key: starts at logon without admin rights.
    User,
    /// Task Scheduler task with highest privileges: starts as administrator without a UAC prompt.
    Elevated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClickAction {
    None,
    #[default]
    Flyout,
    Settings,
    ToggleWidget,
    TaskManager,
}

impl ClickAction {
    pub const ALL: [ClickAction; 5] =
        [ClickAction::None, ClickAction::Flyout, ClickAction::Settings, ClickAction::ToggleWidget, ClickAction::TaskManager];

    pub fn key(self) -> &'static str {
        match self {
            ClickAction::None => "click.none",
            ClickAction::Flyout => "click.flyout",
            ClickAction::Settings => "click.settings",
            ClickAction::ToggleWidget => "click.toggle",
            ClickAction::TaskManager => "click.taskmgr",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetworkUnit {
    #[default]
    Bytes,
    Bits,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// `auto` (Windows display language) or `en`, `ru`, `zh`, `es`, `fr`, `pt`, `de`.
    pub language: String,
    pub refresh_interval_ms: u32,
    /// Samples kept for graphs.
    pub history_length: u32,
    pub autostart: Autostart,
    pub network_unit: NetworkUnit,
    pub left_click: ClickAction,
    pub double_click: ClickAction,
    pub middle_click: ClickAction,
}

impl Default for General {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            refresh_interval_ms: 1000,
            history_length: 60,
            autostart: Autostart::Off,
            network_unit: NetworkUnit::Bytes,
            left_click: ClickAction::Flyout,
            double_click: ClickAction::TaskManager,
            middle_click: ClickAction::Settings,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    /// Follow the taskbar (light/dark) and the Windows accent color.
    #[default]
    System,
    Light,
    Dark,
    /// Use `foreground_color` / `background_color` from the config.
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: ThemeMode,
    pub font_family: String,
    /// 0 = derived from row height.
    pub font_size: f32,
    pub font_bold: bool,
    /// Opacity of labels relative to values (0.1..1).
    pub label_opacity: f32,
    pub foreground_color: String,
    pub background_color: String,
    /// 0 = transparent, the taskbar shows through.
    pub background_opacity: f32,
    pub corner_radius: f32,
    /// Use the Windows accent color; otherwise `accent_color`.
    pub use_system_accent: bool,
    pub accent_color: String,
    /// Give each category (CPU, GPU, memory, network, disk, Claude) its own color.
    pub color_by_category: bool,
    pub use_threshold_colors: bool,
    pub warn_threshold: f32,
    pub crit_threshold: f32,
    pub warn_color: String,
    pub crit_color: String,
    pub padding: f32,
    pub tile_spacing: f32,
    pub graph_width: f32,
    pub bar_width: f32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            font_family: "Segoe UI Variable Text".into(),
            font_size: 0.0,
            font_bold: false,
            label_opacity: 0.7,
            foreground_color: "#FFFFFF".into(),
            background_color: "#202020".into(),
            background_opacity: 0.0,
            corner_radius: 6.0,
            use_system_accent: true,
            accent_color: "#4CC2FF".into(),
            color_by_category: true,
            use_threshold_colors: true,
            warn_threshold: 70.0,
            crit_threshold: 90.0,
            warn_color: "#FFB900".into(),
            crit_color: "#F1707A".into(),
            padding: 4.0,
            tile_spacing: 10.0,
            graph_width: 36.0,
            bar_width: 30.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// Right side, just left of the notification area.
    #[default]
    NearTray,
    Left,
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MonitorMode {
    #[default]
    Primary,
    All,
    /// Only the taskbar on monitor `monitor_index` (0 = primary).
    Specific,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EmbedMode {
    /// Top-most window drawn over the taskbar. Most compatible.
    #[default]
    Overlay,
    /// Window re-parented into the taskbar (experimental, Windows 10 style taskbars work best).
    Embedded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub placement: Placement,
    /// Shift (DIP) away from the anchor edge.
    pub offset_x: f32,
    pub offset_y: f32,
    /// Rows the tiles are stacked into on a horizontal taskbar (1..4).
    pub rows: u32,
    pub monitor: MonitorMode,
    pub monitor_index: u32,
    /// When unlocked the widget can be dragged along the taskbar.
    pub lock_position: bool,
    pub hide_on_fullscreen: bool,
    pub embed: EmbedMode,
    /// Space (DIP) above and below the widget inside the taskbar.
    pub vertical_margin: f32,
    /// Space (DIP) kept on the right of secondary taskbars, where Windows 11 shows the clock.
    pub secondary_right_margin: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            placement: Placement::NearTray,
            offset_x: 0.0,
            offset_y: 0.0,
            rows: 2,
            monitor: MonitorMode::Primary,
            monitor_index: 0,
            lock_position: true,
            hide_on_fullscreen: true,
            embed: EmbedMode::Overlay,
            vertical_margin: 2.0,
            secondary_right_margin: 140.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrayIconMode {
    #[default]
    AppIcon,
    /// Draw the value of `icon_metric` into the tray icon.
    Metric,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tray {
    pub show_icon: bool,
    pub icon_mode: TrayIconMode,
    pub icon_metric: MetricKind,
    pub show_tooltip: bool,
}

impl Default for Tray {
    fn default() -> Self {
        Self { show_icon: true, icon_mode: TrayIconMode::AppIcon, icon_metric: MetricKind::CpuUsage, show_tooltip: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Claude {
    pub enabled: bool,
    /// Claude Code `.credentials.json` (file or directory). Empty = `%CLAUDE_CONFIG_DIR%` or `%USERPROFILE%\.claude`.
    pub credentials_path: String,
    pub poll_interval_secs: u32,
    pub show_reset_countdown: bool,
    pub usage_endpoint: String,
}

impl Default for Claude {
    fn default() -> Self {
        Self {
            enabled: true,
            credentials_path: String::new(),
            poll_interval_secs: 120,
            show_reset_countdown: true,
            usage_endpoint: "https://api.anthropic.com/api/oauth/usage".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Codex {
    pub enabled: bool,
    /// Codex CLI auth.json, or its directory. Empty honors CODEX_HOME.
    pub credentials_path: String,
    pub poll_interval_secs: u32,
    pub show_reset_countdown: bool,
}

impl Default for Codex {
    fn default() -> Self {
        Self { enabled: false, credentials_path: String::new(), poll_interval_secs: 120, show_reset_countdown: true }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ollama {
    pub enabled: bool,
    /// Empty uses OLLAMA_API_KEY. Stored in plaintext when configured here.
    pub api_key: String,
    pub poll_interval_secs: u32,
    pub show_reset_countdown: bool,
}

impl std::fmt::Debug for Ollama {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ollama")
            .field("enabled", &self.enabled)
            .field("api_key", &"<redacted>")
            .field("poll_interval_secs", &self.poll_interval_secs)
            .field("show_reset_countdown", &self.show_reset_countdown)
            .finish()
    }
}

impl Default for Ollama {
    fn default() -> Self {
        Self { enabled: false, api_key: String::new(), poll_interval_secs: 120, show_reset_countdown: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hardware {
    /// GPU temperature/fan/power through the WDDM driver interface (no admin rights required).
    pub gpu_sensors: bool,
    /// CPU temperature through ACPI thermal zones (performance counters; WMI fallback needs admin rights).
    pub thermal_zones: bool,
    /// Read every sensor published by a running LibreHardwareMonitor / OpenHardwareMonitor over WMI.
    pub lhm_bridge: bool,
    /// Which GPUs GPU metrics without an explicit source show.
    pub gpu_display: GpuDisplay,
    /// GPU for `gpu_display = main`: name substring or LUID; empty = adapter with the most dedicated memory.
    pub gpu_adapter: String,
    /// Interval for the slower sensor queries.
    pub sensor_interval_ms: u32,
}

impl Default for Hardware {
    fn default() -> Self {
        Self {
            gpu_sensors: true,
            thermal_zones: true,
            lhm_bridge: true,
            gpu_display: GpuDisplay::Main,
            gpu_adapter: String::new(),
            sensor_interval_ms: 2000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GpuDisplay {
    /// One GPU: `gpu_adapter`, or the one with the most video memory (the most powerful).
    #[default]
    Main,
    /// One GPU: whichever is busiest at the moment.
    Busiest,
    /// Every GPU gets its own tile (`GPU1`, `GPU2`…).
    All,
}

impl GpuDisplay {
    pub const ALL: [GpuDisplay; 3] = [GpuDisplay::Main, GpuDisplay::Busiest, GpuDisplay::All];

    pub fn key(self) -> &'static str {
        match self {
            GpuDisplay::Main => "gpu.main",
            GpuDisplay::Busiest => "gpu.busiest",
            GpuDisplay::All => "gpu.all",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "RawMetricConfig")]
pub struct MetricConfig {
    pub kind: MetricKind,
    /// Meaning depends on the kind: GPU name, adapter name (`*` = all), disk instance (`0 C:`),
    /// drive (`D:`), sensor identifier or Claude model (`fable`, `opus`…). Empty = automatic.
    pub source: String,
    /// Empty = default label of the kind.
    pub label: String,
    pub style: MetricStyle,
    pub show_in_widget: bool,
    pub show_in_flyout: bool,
    pub show_label: bool,
    pub show_value: bool,
    /// Graph and ring styles: draw text over the graph instead of beside it.
    pub overlay_text: bool,
    /// `#RRGGBB`; empty = automatic.
    pub color: String,
    /// Width (DIP) of the graph/bar part; 0 = appearance default.
    pub width: f32,
    /// -1 = automatic.
    pub decimals: i32,
    /// Scale for rates/sensors; 0 = auto-scale to the visible history.
    pub max_value: f64,
    /// Hide the tile while there is no data (e.g. a Claude model limit your plan does not have).
    pub hide_when_unavailable: bool,
}

/// Same as [`MetricConfig`], but kind-dependent fields are optional so their defaults can follow `kind`.
#[derive(Deserialize)]
#[serde(default)]
struct RawMetricConfig {
    kind: MetricKind,
    source: String,
    label: String,
    style: Option<MetricStyle>,
    show_in_widget: bool,
    show_in_flyout: bool,
    show_label: bool,
    show_value: bool,
    overlay_text: bool,
    color: String,
    width: f32,
    decimals: i32,
    max_value: f64,
    hide_when_unavailable: Option<bool>,
}

impl Default for RawMetricConfig {
    fn default() -> Self {
        let d = MetricConfig::default();
        Self {
            kind: d.kind,
            source: d.source,
            label: d.label,
            style: None,
            show_in_widget: d.show_in_widget,
            show_in_flyout: d.show_in_flyout,
            show_label: d.show_label,
            show_value: d.show_value,
            overlay_text: d.overlay_text,
            color: d.color,
            width: d.width,
            decimals: d.decimals,
            max_value: d.max_value,
            hide_when_unavailable: None,
        }
    }
}

impl From<RawMetricConfig> for MetricConfig {
    fn from(r: RawMetricConfig) -> Self {
        let base = MetricConfig::new(r.kind);
        Self {
            kind: r.kind,
            source: r.source,
            label: r.label,
            style: r.style.unwrap_or(base.style),
            show_in_widget: r.show_in_widget,
            show_in_flyout: r.show_in_flyout,
            show_label: r.show_label,
            show_value: r.show_value,
            overlay_text: r.overlay_text,
            color: r.color,
            width: r.width,
            decimals: r.decimals,
            max_value: r.max_value,
            hide_when_unavailable: r.hide_when_unavailable.unwrap_or(base.hide_when_unavailable),
        }
    }
}

impl Default for MetricConfig {
    fn default() -> Self {
        Self::new(MetricKind::CpuUsage)
    }
}

impl MetricConfig {
    pub fn new(kind: MetricKind) -> Self {
        Self {
            kind,
            source: String::new(),
            label: String::new(),
            style: kind.def().default_style,
            show_in_widget: true,
            show_in_flyout: true,
            show_label: true,
            show_value: true,
            overlay_text: false,
            color: String::new(),
            width: 0.0,
            decimals: -1,
            max_value: 0.0,
            hide_when_unavailable: matches!(
                kind,
                MetricKind::ClaudeModel | MetricKind::ClaudeExtra | MetricKind::CpuTemp | MetricKind::GpuTemp
            ),
        }
    }

    pub fn styled(kind: MetricKind, style: MetricStyle) -> Self {
        Self { style, ..Self::new(kind) }
    }

    pub fn flyout_only(mut self) -> Self {
        self.show_in_widget = false;
        self
    }

    pub fn with_source(mut self, source: &str) -> Self {
        self.source = source.into();
        self
    }
}

impl Settings {
    /// Clamps values into valid ranges.
    pub fn normalize(&mut self) {
        let g = &mut self.general;
        g.refresh_interval_ms = g.refresh_interval_ms.clamp(250, 60_000);
        g.history_length = g.history_length.clamp(10, 3600);
        if g.language != "auto" && crate::i18n::Lang::from_code(&g.language).is_none() {
            g.language = "auto".into();
        }

        let a = &mut self.appearance;
        if a.font_family.trim().is_empty() {
            a.font_family = Appearance::default().font_family;
        }
        a.font_size = if a.font_size <= 0.0 { 0.0 } else { a.font_size.clamp(6.0, 40.0) };
        a.label_opacity = a.label_opacity.clamp(0.1, 1.0);
        a.background_opacity = a.background_opacity.clamp(0.0, 1.0);
        a.corner_radius = a.corner_radius.clamp(0.0, 30.0);
        a.warn_threshold = a.warn_threshold.clamp(0.0, 100.0);
        a.crit_threshold = a.crit_threshold.clamp(a.warn_threshold, 100.0);
        a.padding = a.padding.clamp(0.0, 30.0);
        a.tile_spacing = a.tile_spacing.clamp(0.0, 60.0);
        a.graph_width = a.graph_width.clamp(8.0, 400.0);
        a.bar_width = a.bar_width.clamp(4.0, 400.0);

        let l = &mut self.layout;
        l.rows = l.rows.clamp(1, 4);
        l.vertical_margin = l.vertical_margin.clamp(0.0, 20.0);
        l.secondary_right_margin = l.secondary_right_margin.clamp(0.0, 1000.0);
        l.offset_x = l.offset_x.clamp(-10_000.0, 10_000.0);
        l.offset_y = l.offset_y.clamp(-100.0, 100.0);

        let c = &mut self.claude;
        c.poll_interval_secs = c.poll_interval_secs.clamp(30, 3600);
        if c.usage_endpoint.trim().is_empty() {
            c.usage_endpoint = Claude::default().usage_endpoint;
        }
        self.codex.poll_interval_secs = self.codex.poll_interval_secs.clamp(60, 3600);
        self.ollama.poll_interval_secs = self.ollama.poll_interval_secs.clamp(60, 3600);

        self.hardware.sensor_interval_ms = self.hardware.sensor_interval_ms.clamp(500, 60_000);
        // Earlier versions stored "busiest" as the `*` adapter.
        if self.hardware.gpu_adapter.trim() == "*" {
            self.hardware.gpu_adapter.clear();
            self.hardware.gpu_display = GpuDisplay::Busiest;
        }

        for m in &mut self.metrics {
            m.width = m.width.clamp(0.0, 400.0);
            m.decimals = m.decimals.clamp(-1, 3);
            m.max_value = m.max_value.max(0.0);
        }
    }

    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        let mut s: Settings = toml::from_str(text)?;
        s.normalize();
        Ok(s)
    }

    pub fn to_toml(&self) -> String {
        let body = toml::to_string_pretty(self).unwrap_or_default();
        format!(
            "# Taskbar Monitor settings. Edit here or via the Settings window; changes apply live.\n\
             # Documentation: https://github.com/noartem/taskbar-monitor-widget#configuration\n\n{body}"
        )
    }

    /// Metrics that need to be collected: everything visible in the widget, flyout or tray icon.
    pub fn required_kinds(&self, include_flyout: bool) -> Vec<MetricKind> {
        let mut kinds: Vec<MetricKind> =
            self.metrics.iter().filter(|m| m.show_in_widget || (include_flyout && m.show_in_flyout)).map(|m| m.kind).collect();
        if self.tray.icon_mode == TrayIconMode::Metric {
            kinds.push(self.tray.icon_metric);
        }
        kinds
    }
}

/// Where the application keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub portable: bool,
    pub data_dir: PathBuf,
}

impl AppPaths {
    /// Portable when `portable.txt` is next to the exe or `--portable` is passed; otherwise `%APPDATA%\TaskbarMonitor`.
    pub fn resolve(exe_dir: &Path, args: &[String], appdata: Option<PathBuf>) -> Self {
        let portable = args.iter().any(|a| a.eq_ignore_ascii_case("--portable")) || exe_dir.join(PORTABLE_MARKER).exists();
        if portable {
            return Self { portable: true, data_dir: exe_dir.to_path_buf() };
        }
        let base = appdata.unwrap_or_else(|| exe_dir.to_path_buf());
        Self { portable: false, data_dir: base.join(APP_DIR) }
    }

    pub fn config_file(&self) -> PathBuf {
        self.data_dir.join(CONFIG_FILE)
    }

    pub fn log_file(&self) -> PathBuf {
        self.data_dir.join("taskbar-monitor.log")
    }
}

#[derive(Debug)]
pub enum LoadOutcome {
    Loaded,
    Created,
    /// The file could not be parsed; it was backed up and defaults are used.
    Recovered {
        backup: PathBuf,
        error: String,
    },
}

/// Loads settings, creating the file when missing. A broken file is backed up, never silently overwritten.
pub fn load(path: &Path) -> (Settings, LoadOutcome) {
    match std::fs::read_to_string(path) {
        Ok(text) => match Settings::from_toml(&text) {
            Ok(s) => (s, LoadOutcome::Loaded),
            Err(e) => {
                let backup = path.with_extension("toml.broken");
                let _ = std::fs::copy(path, &backup);
                (Settings::default(), LoadOutcome::Recovered { backup, error: e.to_string() })
            }
        },
        Err(_) => {
            let s = Settings::default();
            let _ = save(path, &s);
            (s, LoadOutcome::Created)
        }
    }
}

/// Atomic save: write a temp file, then rename over the original.
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, settings.to_toml())?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrips_through_toml() {
        let s = Settings::default();
        let parsed = Settings::from_toml(&s.to_toml()).unwrap();
        assert_eq!(s, parsed);
    }

    #[test]
    fn partial_file_uses_defaults() {
        let s = Settings::from_toml("[layout]\nrows = 3\n").unwrap();
        assert_eq!(s.layout.rows, 3);
        assert_eq!(s.general.refresh_interval_ms, 1000);
        assert!(!s.metrics.is_empty());
    }

    #[test]
    fn values_are_clamped() {
        let s = Settings::from_toml("[general]\nrefresh_interval_ms = 5\n[layout]\nrows = 99\n").unwrap();
        assert_eq!(s.general.refresh_interval_ms, 250);
        assert_eq!(s.layout.rows, 4);
    }

    #[test]
    fn metrics_parse_from_array_of_tables() {
        let s = Settings::from_toml(
            "[[metrics]]\nkind = \"claude_model\"\nsource = \"fable\"\nstyle = \"ring\"\n[[metrics]]\nkind = \"net_down\"\n",
        )
        .unwrap();
        assert_eq!(s.metrics.len(), 2);
        assert_eq!(s.metrics[0].kind, MetricKind::ClaudeModel);
        assert_eq!(s.metrics[0].style, MetricStyle::Ring);
        assert_eq!(s.metrics[1].style, MetricStyle::Text);
    }

    #[test]
    fn portable_marker_switches_data_dir() {
        let dir = std::env::temp_dir().join(format!("tm-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let installed = AppPaths::resolve(&dir, &[], Some(PathBuf::from("/appdata")));
        assert!(!installed.portable);
        assert_eq!(installed.data_dir, PathBuf::from("/appdata").join(APP_DIR));

        std::fs::write(dir.join(PORTABLE_MARKER), "").unwrap();
        let portable = AppPaths::resolve(&dir, &[], Some(PathBuf::from("/appdata")));
        assert!(portable.portable);
        assert_eq!(portable.data_dir, dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

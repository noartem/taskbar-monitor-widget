//! Metric kinds, display styles and their static properties.

use serde::{Deserialize, Serialize};

/// What a metric tile measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    #[default]
    CpuUsage,
    CpuTemp,
    RamUsage,
    GpuUsage,
    GpuTemp,
    VramUsage,
    NetDown,
    NetUp,
    NetTotal,
    DiskRead,
    DiskWrite,
    DiskActive,
    DiskSpace,
    ClaudeSession,
    ClaudeWeekly,
    ClaudeModel,
    ClaudeExtra,
    CodexSession,
    CodexWeekly,
    OllamaMonthly,
    OllamaSession,
    OllamaWeekly,
    Sensor,
}

impl MetricKind {
    pub const ALL: [MetricKind; 23] = [
        MetricKind::CpuUsage,
        MetricKind::CpuTemp,
        MetricKind::RamUsage,
        MetricKind::GpuUsage,
        MetricKind::GpuTemp,
        MetricKind::VramUsage,
        MetricKind::NetDown,
        MetricKind::NetUp,
        MetricKind::NetTotal,
        MetricKind::DiskRead,
        MetricKind::DiskWrite,
        MetricKind::DiskActive,
        MetricKind::DiskSpace,
        MetricKind::ClaudeSession,
        MetricKind::ClaudeWeekly,
        MetricKind::ClaudeModel,
        MetricKind::ClaudeExtra,
        MetricKind::CodexSession,
        MetricKind::CodexWeekly,
        MetricKind::OllamaMonthly,
        MetricKind::OllamaSession,
        MetricKind::OllamaWeekly,
        MetricKind::Sensor,
    ];

    /// Stable identifier, also used as the i18n key suffix (`kind.cpu_usage`).
    pub fn key(self) -> &'static str {
        match self {
            MetricKind::CpuUsage => "cpu_usage",
            MetricKind::CpuTemp => "cpu_temp",
            MetricKind::RamUsage => "ram_usage",
            MetricKind::GpuUsage => "gpu_usage",
            MetricKind::GpuTemp => "gpu_temp",
            MetricKind::VramUsage => "vram_usage",
            MetricKind::NetDown => "net_down",
            MetricKind::NetUp => "net_up",
            MetricKind::NetTotal => "net_total",
            MetricKind::DiskRead => "disk_read",
            MetricKind::DiskWrite => "disk_write",
            MetricKind::DiskActive => "disk_active",
            MetricKind::DiskSpace => "disk_space",
            MetricKind::ClaudeSession => "claude_session",
            MetricKind::ClaudeWeekly => "claude_weekly",
            MetricKind::ClaudeModel => "claude_model",
            MetricKind::ClaudeExtra => "claude_extra",
            MetricKind::CodexSession => "codex_session",
            MetricKind::CodexWeekly => "codex_weekly",
            MetricKind::OllamaMonthly => "ollama_monthly",
            MetricKind::OllamaSession => "ollama_session",
            MetricKind::OllamaWeekly => "ollama_weekly",
            MetricKind::Sensor => "sensor",
        }
    }

    pub fn def(self) -> MetricDef {
        use MetricCategory as C;
        use MetricStyle as S;
        use MetricUnit as U;
        use SourceType as T;
        let (unit, label, style, source, template, category) = match self {
            MetricKind::CpuUsage => (U::Percent, "CPU", S::Line, T::None, "100%", C::Cpu),
            MetricKind::CpuTemp => (U::Celsius, "CPU°", S::Text, T::None, "100°C", C::Cpu),
            MetricKind::RamUsage => (U::Percent, "RAM", S::Bar, T::None, "100%", C::Memory),
            MetricKind::GpuUsage => (U::Percent, "GPU", S::Line, T::Gpu, "100%", C::Gpu),
            MetricKind::GpuTemp => (U::Celsius, "GPU°", S::Text, T::Gpu, "100°C", C::Gpu),
            MetricKind::VramUsage => (U::Percent, "VRAM", S::Bar, T::Gpu, "100%", C::Gpu),
            MetricKind::NetDown => (U::BytesPerSecond, "↓", S::Text, T::Network, "999.9 MB/s", C::Network),
            MetricKind::NetUp => (U::BytesPerSecond, "↑", S::Text, T::Network, "999.9 MB/s", C::Network),
            MetricKind::NetTotal => (U::BytesPerSecond, "⇅", S::Area, T::Network, "999.9 MB/s", C::Network),
            MetricKind::DiskRead => (U::BytesPerSecond, "R", S::Text, T::Disk, "999.9 MB/s", C::Disk),
            MetricKind::DiskWrite => (U::BytesPerSecond, "W", S::Text, T::Disk, "999.9 MB/s", C::Disk),
            MetricKind::DiskActive => (U::Percent, "DISK", S::Line, T::Disk, "100%", C::Disk),
            MetricKind::DiskSpace => (U::Percent, "C:", S::Bar, T::Drive, "100%", C::Disk),
            MetricKind::ClaudeSession => (U::Percent, "5h", S::Bar, T::None, "100%", C::Claude),
            MetricKind::ClaudeWeekly => (U::Percent, "7d", S::Bar, T::None, "100%", C::Claude),
            MetricKind::ClaudeModel => (U::Percent, "Model", S::Bar, T::ClaudeLimit, "100%", C::Claude),
            MetricKind::ClaudeExtra => (U::Percent, "$", S::Bar, T::None, "100%", C::Claude),
            MetricKind::CodexSession => (U::Percent, "Codex 5h", S::Bar, T::None, "100%", C::Codex),
            MetricKind::CodexWeekly => (U::Percent, "Codex 7d", S::Bar, T::None, "100%", C::Codex),
            MetricKind::OllamaMonthly => (U::Percent, "Ollama $", S::Bar, T::None, "100%", C::Ollama),
            MetricKind::OllamaSession => (U::Percent, "Ollama 5h", S::Bar, T::None, "100%", C::Ollama),
            MetricKind::OllamaWeekly => (U::Percent, "Ollama 7d", S::Bar, T::None, "100%", C::Ollama),
            MetricKind::Sensor => (U::Generic, "Sensor", S::Text, T::Sensor, "9999.9", C::Sensors),
        };
        MetricDef { unit, default_label: label, default_style: style, source_type: source, width_template: template, category }
    }

    /// Which providers must run for this metric.
    pub fn data_sources(self) -> DataSources {
        match self {
            MetricKind::CpuUsage => DataSources::CPU,
            MetricKind::CpuTemp => DataSources::SENSORS,
            MetricKind::RamUsage => DataSources::MEMORY,
            MetricKind::GpuUsage | MetricKind::VramUsage => DataSources::GPU,
            MetricKind::GpuTemp => DataSources::GPU | DataSources::SENSORS,
            MetricKind::NetDown | MetricKind::NetUp | MetricKind::NetTotal => DataSources::NETWORK,
            MetricKind::DiskRead | MetricKind::DiskWrite | MetricKind::DiskActive => DataSources::DISK,
            MetricKind::DiskSpace => DataSources::DRIVES,
            MetricKind::ClaudeSession | MetricKind::ClaudeWeekly | MetricKind::ClaudeModel | MetricKind::ClaudeExtra => {
                DataSources::CLAUDE
            }
            MetricKind::CodexSession | MetricKind::CodexWeekly => DataSources::CODEX,
            MetricKind::OllamaMonthly | MetricKind::OllamaSession | MetricKind::OllamaWeekly => DataSources::OLLAMA,
            MetricKind::Sensor => DataSources::SENSORS,
        }
    }
}

/// How a metric is drawn in the taskbar widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MetricStyle {
    /// Label and value as text.
    #[default]
    Text,
    /// Horizontal progress bar between label and value.
    Bar,
    /// Thin vertical level bar.
    VBar,
    /// Text over a background fill proportional to the value.
    Fill,
    /// History as a line chart.
    Line,
    /// History as a filled area chart.
    Area,
    /// History as columns.
    Columns,
    /// Circular gauge.
    Ring,
}

impl MetricStyle {
    pub const ALL: [MetricStyle; 8] = [
        MetricStyle::Text,
        MetricStyle::Bar,
        MetricStyle::VBar,
        MetricStyle::Fill,
        MetricStyle::Line,
        MetricStyle::Area,
        MetricStyle::Columns,
        MetricStyle::Ring,
    ];

    pub fn key(self) -> &'static str {
        match self {
            MetricStyle::Text => "text",
            MetricStyle::Bar => "bar",
            MetricStyle::VBar => "v_bar",
            MetricStyle::Fill => "fill",
            MetricStyle::Line => "line",
            MetricStyle::Area => "area",
            MetricStyle::Columns => "columns",
            MetricStyle::Ring => "ring",
        }
    }

    pub fn is_graph(self) -> bool {
        matches!(self, MetricStyle::Line | MetricStyle::Area | MetricStyle::Columns)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricUnit {
    Percent,
    BytesPerSecond,
    Celsius,
    Generic,
}

/// What the `source` field of a metric selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType {
    None,
    Gpu,
    Network,
    Disk,
    Drive,
    Sensor,
    ClaudeLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetricCategory {
    Cpu,
    Memory,
    Gpu,
    Network,
    Disk,
    Claude,
    Codex,
    Ollama,
    Sensors,
}

#[derive(Debug, Clone, Copy)]
pub struct MetricDef {
    pub unit: MetricUnit,
    pub default_label: &'static str,
    pub default_style: MetricStyle,
    pub source_type: SourceType,
    /// Widest text the value is expected to take; keeps tile widths stable while values change.
    pub width_template: &'static str,
    pub category: MetricCategory,
}

/// Bit set of data providers, so the collector can skip what nobody displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct DataSources(pub u32);

impl DataSources {
    pub const NONE: Self = Self(0);
    pub const CPU: Self = Self(1);
    pub const MEMORY: Self = Self(2);
    pub const GPU: Self = Self(4);
    pub const NETWORK: Self = Self(8);
    pub const DISK: Self = Self(16);
    pub const DRIVES: Self = Self(32);
    pub const SENSORS: Self = Self(64);
    pub const CLAUDE: Self = Self(128);
    pub const CODEX: Self = Self(256);
    pub const OLLAMA: Self = Self(512);
    pub const ALL: Self = Self(1023);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0 && other.0 != 0
    }

    pub fn from_kinds(kinds: impl IntoIterator<Item = MetricKind>) -> Self {
        kinds.into_iter().fold(Self::NONE, |acc, k| acc | k.data_sources())
    }
}

impl std::ops::BitOr for DataSources {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for DataSources {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

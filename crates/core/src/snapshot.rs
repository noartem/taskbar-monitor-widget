//! Everything the providers measured during one refresh tick.

use std::collections::BTreeMap;

use crate::claude::ClaudeUsage;

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub cpu_usage: Option<f64>,
    pub cpu_temperature: Option<f64>,
    pub cpu_name: Option<String>,
    pub ram_used: u64,
    pub ram_total: u64,
    pub gpus: Vec<GpuInfo>,
    pub networks: Vec<NetworkInfo>,
    pub disks: Vec<DiskInfo>,
    pub drives: Vec<DriveInfo>,
    /// Keyed by sensor identifier.
    pub sensors: BTreeMap<String, SensorInfo>,
    pub claude: ClaudeUsage,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuInfo {
    pub name: String,
    /// `0x00000000_0x0000D1B6` (high_low), matches performance counter instance names.
    pub luid: String,
    /// Busiest engine type, like Task Manager (0..100).
    pub usage: Option<f64>,
    pub vram_used: u64,
    pub vram_total: u64,
    pub temperature: Option<f64>,
    pub fan_rpm: Option<u32>,
    /// Adapter with the most dedicated memory.
    pub is_default: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetworkInfo {
    pub name: String,
    pub description: String,
    /// Bytes per second.
    pub down: f64,
    pub up: f64,
    /// Physical adapter (not a virtual switch, VPN or filter driver).
    pub is_hardware: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiskInfo {
    /// Performance counter instance: `0 C:`, `1 D: E:` or `_Total`.
    pub name: String,
    pub read: f64,
    pub write: f64,
    /// Active time, 0..100.
    pub active: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DriveInfo {
    /// `C:`
    pub name: String,
    pub used: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SensorInfo {
    pub id: String,
    pub hardware: String,
    pub name: String,
    /// `Temperature`, `Load`, `Fan`, …
    pub kind: String,
    pub value: f64,
    /// `°C`, `%`, `RPM`, `W`, `V`, `MHz`, …
    pub unit: String,
}

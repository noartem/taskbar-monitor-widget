//! Performance counters (the same data Task Manager shows), read through one PDH query per tick.

use std::collections::HashMap;

use tm_core::snapshot::DiskInfo;
use windows::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
    PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterArrayW, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::core::{HSTRING, PCWSTR};

/// Do not cap values at 100 (CPU utility can exceed 100% with turbo; we clamp ourselves).
const PDH_FMT_NOCAP100: u32 = 0x8000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PdhNeeds {
    pub cpu: bool,
    pub gpu: bool,
    pub disk: bool,
    pub thermal: bool,
}

pub struct Pdh {
    query: PDH_HQUERY,
    pub needs: PdhNeeds,
    cpu: Option<PDH_HCOUNTER>,
    gpu_engine: Option<PDH_HCOUNTER>,
    gpu_memory: Option<PDH_HCOUNTER>,
    disk_read: Option<PDH_HCOUNTER>,
    disk_write: Option<PDH_HCOUNTER>,
    disk_idle: Option<PDH_HCOUNTER>,
    thermal: Option<(PDH_HCOUNTER, bool)>,
}

impl Pdh {
    pub fn open(needs: PdhNeeds) -> Option<Self> {
        let mut query = PDH_HQUERY::default();
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
            crate::log!("PdhOpenQuery failed");
            return None;
        }
        let add = |path: &str| -> Option<PDH_HCOUNTER> {
            let mut counter = PDH_HCOUNTER::default();
            let status = unsafe { PdhAddEnglishCounterW(query, &HSTRING::from(path), 0, &mut counter) };
            if status == 0 {
                Some(counter)
            } else {
                crate::log!("counter unavailable: {path} (0x{status:08X})");
                None
            }
        };
        let mut pdh = Self {
            query,
            needs,
            cpu: None,
            gpu_engine: None,
            gpu_memory: None,
            disk_read: None,
            disk_write: None,
            disk_idle: None,
            thermal: None,
        };
        if needs.cpu {
            pdh.cpu = add(r"\Processor Information(_Total)\% Processor Utility")
                .or_else(|| add(r"\Processor(_Total)\% Processor Time"));
        }
        if needs.gpu {
            pdh.gpu_engine = add(r"\GPU Engine(*)\Utilization Percentage");
            pdh.gpu_memory = add(r"\GPU Adapter Memory(*)\Dedicated Usage");
        }
        if needs.disk {
            pdh.disk_read = add(r"\PhysicalDisk(*)\Disk Read Bytes/sec");
            pdh.disk_write = add(r"\PhysicalDisk(*)\Disk Write Bytes/sec");
            pdh.disk_idle = add(r"\PhysicalDisk(*)\% Idle Time");
        }
        if needs.thermal {
            pdh.thermal = add(r"\Thermal Zone Information(*)\High Precision Temperature")
                .map(|c| (c, true))
                .or_else(|| add(r"\Thermal Zone Information(*)\Temperature").map(|c| (c, false)));
        }
        Some(pdh)
    }

    pub fn collect(&self) {
        unsafe { PdhCollectQueryData(self.query) };
    }

    fn valid(v: &PDH_FMT_COUNTERVALUE) -> bool {
        v.CStatus == PDH_CSTATUS_VALID_DATA || v.CStatus == PDH_CSTATUS_NEW_DATA
    }

    fn single(counter: PDH_HCOUNTER) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        let status = unsafe { PdhGetFormattedCounterValue(counter, PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100), None, &mut v) };
        (status == 0 && Self::valid(&v)).then_some(unsafe { v.Anonymous.doubleValue })
    }

    /// All instances of a wildcard counter as (instance name, value).
    fn array(counter: PDH_HCOUNTER, large: bool) -> Vec<(String, f64)> {
        let format = if large { PDH_FMT_LARGE } else { PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100) };
        let item = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        // Instances (e.g. per-process GPU engines) can appear between the sizing call and the read; retry then.
        for _ in 0..3 {
            let mut size = 0u32;
            let mut count = 0u32;
            unsafe {
                let status = PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, None);
                if status != PDH_MORE_DATA {
                    log_status("PdhGetFormattedCounterArray (size)", status);
                    return Vec::new();
                }
                // Instance names are stored after the items in the same buffer; allocate items for alignment.
                let n = (size as usize).div_ceil(item) + 1;
                let mut buf = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); n];
                size = (n * item) as u32;
                let status = PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, Some(buf.as_mut_ptr()));
                if status == PDH_MORE_DATA {
                    continue;
                }
                if status != 0 {
                    log_status("PdhGetFormattedCounterArray", status);
                    return Vec::new();
                }
                return buf[..(count as usize).min(n)]
                    .iter()
                    .filter(|it| Self::valid(&it.FmtValue))
                    .map(|it| {
                        let name = it.szName.to_string().unwrap_or_default();
                        let value =
                            if large { it.FmtValue.Anonymous.largeValue as f64 } else { it.FmtValue.Anonymous.doubleValue };
                        (name, value)
                    })
                    .collect();
            }
        }
        Vec::new()
    }

    pub fn cpu(&self) -> Option<f64> {
        self.cpu.and_then(Self::single).map(|v| v.clamp(0.0, 100.0))
    }

    /// GPU utilization per adapter LUID, computed like Task Manager: for every engine type sum all processes,
    /// then take the busiest engine type.
    /// `None` when the counter is not available on this system.
    pub fn gpu_usage(&self) -> Option<HashMap<String, f64>> {
        let c = self.gpu_engine?;
        let mut per_engine: HashMap<(String, String), f64> = HashMap::new();
        for (name, v) in Self::array(c, false) {
            if let (Some(luid), Some(engine)) = (instance_luid(&name), name.rsplit("engtype_").next()) {
                *per_engine.entry((luid, engine.to_string())).or_default() += v;
            }
        }
        let mut result: HashMap<String, f64> = HashMap::new();
        for ((luid, _), v) in per_engine {
            let e = result.entry(luid).or_default();
            *e = e.max(v.min(100.0));
        }
        Some(result)
    }

    /// Dedicated video memory in use per adapter LUID; `None` when the counter is not available.
    pub fn gpu_dedicated(&self) -> Option<HashMap<String, u64>> {
        let c = self.gpu_memory?;
        let mut result = HashMap::new();
        for (name, v) in Self::array(c, true) {
            if let Some(luid) = instance_luid(&name) {
                *result.entry(luid).or_default() += v.max(0.0) as u64;
            }
        }
        Some(result)
    }

    pub fn disks(&self) -> Vec<DiskInfo> {
        let (Some(r), Some(w), Some(i)) = (self.disk_read, self.disk_write, self.disk_idle) else { return Vec::new() };
        let reads: HashMap<String, f64> = Self::array(r, false).into_iter().collect();
        let writes: HashMap<String, f64> = Self::array(w, false).into_iter().collect();
        let mut disks: Vec<DiskInfo> = Self::array(i, false)
            .into_iter()
            .map(|(name, idle)| DiskInfo {
                read: reads.get(&name).copied().unwrap_or(0.0),
                write: writes.get(&name).copied().unwrap_or(0.0),
                active: (100.0 - idle).clamp(0.0, 100.0),
                name,
            })
            .collect();
        disks.sort_by(|a, b| a.name.cmp(&b.name));
        disks
    }

    /// Hottest ACPI thermal zone in °C.
    pub fn thermal_celsius(&self) -> Option<f64> {
        let (c, high_precision) = self.thermal?;
        Self::array(c, false)
            .into_iter()
            .map(|(_, v)| if high_precision { v / 10.0 - 273.15 } else { v - 273.15 })
            .filter(|t| (1.0..150.0).contains(t))
            .fold(None, |acc: Option<f64>, t| Some(acc.map_or(t, |a| a.max(t))))
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// `pid_1_luid_0x00000000_0x0000D1B6_phys_0_eng_0_engtype_3D` → `0x00000000_0x0000D1B6`.
pub fn instance_luid(name: &str) -> Option<String> {
    let start = name.find("luid_")? + 5;
    let rest = &name[start..];
    let end = rest.find("_phys").unwrap_or(rest.len());
    Some(rest[..end].to_ascii_uppercase().replace("0X", "0x"))
}

/// Logs an unexpected PDH status at most once a minute (they repeat every tick).
fn log_status(what: &str, status: u32) {
    use std::sync::atomic::{AtomicI64, Ordering};
    static LAST: AtomicI64 = AtomicI64::new(0);
    let now = tm_core::time::now_unix();
    if now - LAST.load(Ordering::Relaxed) >= 60 {
        LAST.store(now, Ordering::Relaxed);
        crate::log!("{what} failed: 0x{status:08X}");
    }
}

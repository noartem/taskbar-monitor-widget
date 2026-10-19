//! Memory, drives and CPU name.

use tm_core::snapshot::DriveInfo;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;
use windows::core::HSTRING;

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;

/// (used, total) physical memory in bytes.
pub fn memory() -> Option<(u64, u64)> {
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut m).ok()? };
    Some((m.ullTotalPhys.saturating_sub(m.ullAvailPhys), m.ullTotalPhys))
}

pub fn drives() -> Vec<DriveInfo> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .filter_map(|i| {
            let name = format!("{}:", (b'A' + i) as char);
            let root = HSTRING::from(format!("{name}\\"));
            let kind = unsafe { GetDriveTypeW(&root) };
            if kind != DRIVE_FIXED && kind != DRIVE_REMOVABLE {
                return None;
            }
            let (mut total, mut free) = (0u64, 0u64);
            unsafe { GetDiskFreeSpaceExW(&root, None, Some(&mut total), Some(&mut free)).ok()? };
            (total > 0).then(|| DriveInfo { name, used: total - free, total })
        })
        .collect()
}

pub fn cpu_name() -> Option<String> {
    crate::theme::hklm_string(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString")
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// CPU load from `GetSystemTimes` deltas; used when performance counters are unavailable.
#[derive(Default)]
pub struct CpuTimes {
    previous: Option<(u64, u64)>,
}

impl CpuTimes {
    pub fn sample(&mut self) -> Option<f64> {
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()? };
        let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
        // Kernel time includes idle time.
        let (idle, total) = (ft(idle), ft(kernel) + ft(user));
        let result = self.previous.and_then(|(pi, pt)| {
            let dt = total.saturating_sub(pt);
            (dt > 0).then(|| (100.0 * (1.0 - idle.saturating_sub(pi) as f64 / dt as f64)).clamp(0.0, 100.0))
        });
        self.previous = Some((idle, total));
        result
    }
}

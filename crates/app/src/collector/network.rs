//! Network throughput from interface octet counters (IP Helper).

use std::collections::HashMap;
use std::time::Instant;

use tm_core::snapshot::NetworkInfo;
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;

const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const IF_TYPE_TUNNEL: u32 = 131;

pub struct Interface {
    pub luid: u64,
    pub name: String,
    pub description: String,
    pub in_octets: u64,
    pub out_octets: u64,
    pub is_hardware: bool,
}

/// Connected interfaces, excluding loopback, tunnels and filter drivers (which duplicate their base adapter).
pub fn interfaces() -> Vec<Interface> {
    let mut out = Vec::new();
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).is_err() || table.is_null() {
            return out;
        }
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        for row in rows {
            let flags = row.InterfaceAndOperStatusFlags._bitfield;
            let hardware = flags & 0x1 != 0;
            let filter = flags & 0x2 != 0;
            if row.OperStatus != IfOperStatusUp || filter || row.Type == IF_TYPE_SOFTWARE_LOOPBACK || row.Type == IF_TYPE_TUNNEL {
                continue;
            }
            out.push(Interface {
                luid: row.InterfaceLuid.Value,
                name: crate::util::from_wide(&row.Alias),
                description: crate::util::from_wide(&row.Description),
                in_octets: row.InOctets,
                out_octets: row.OutOctets,
                is_hardware: hardware,
            });
        }
        FreeMibTable(table as *const _);
    }
    out
}

#[derive(Default)]
pub struct NetMonitor {
    previous: HashMap<u64, (u64, u64)>,
    last: Option<Instant>,
}

impl NetMonitor {
    pub fn sample(&mut self) -> Vec<NetworkInfo> {
        let now = Instant::now();
        let elapsed = self.last.map(|l| now.duration_since(l).as_secs_f64()).unwrap_or(0.0);
        self.last = Some(now);
        let mut next = HashMap::new();
        let result = interfaces()
            .into_iter()
            .map(|i| {
                next.insert(i.luid, (i.in_octets, i.out_octets));
                let (down, up) = match self.previous.get(&i.luid) {
                    Some(&(pin, pout)) if elapsed > 0.0 => {
                        (i.in_octets.saturating_sub(pin) as f64 / elapsed, i.out_octets.saturating_sub(pout) as f64 / elapsed)
                    }
                    _ => (0.0, 0.0),
                };
                NetworkInfo { name: i.name, description: i.description, down, up, is_hardware: i.is_hardware }
            })
            .collect();
        self.previous = next;
        result
    }
}

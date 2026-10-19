//! GPU adapters (DXGI) and driver-reported sensors (D3DKMT perf data — what Task Manager uses for GPU temperature).

use std::collections::HashMap;

use windows::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTER_PERFDATA, D3DKMT_CLOSEADAPTER, D3DKMT_OPENADAPTERFROMLUID, D3DKMT_QUERYADAPTERINFO, D3DKMTCloseAdapter,
    D3DKMTOpenAdapterFromLuid, D3DKMTQueryAdapterInfo, KMTQAITYPE_ADAPTERPERFDATA,
};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};

#[derive(Debug, Clone)]
pub struct Adapter {
    pub name: String,
    pub luid: LUID,
    /// `0x00000000_0x0000D1B6`, matches performance counter instances.
    pub luid_str: String,
    pub dedicated: u64,
}

pub fn luid_string(luid: LUID) -> String {
    format!("0x{:08X}_0x{:08X}", luid.HighPart as u32, luid.LowPart)
}

/// Hardware adapters, excluding software renderers.
pub fn adapters() -> Vec<Adapter> {
    let mut out = Vec::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(desc) = adapter.GetDesc1() else { continue };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || desc.VendorId == 0x1414 {
                continue;
            }
            if out
                .iter()
                .any(|a: &Adapter| a.luid.LowPart == desc.AdapterLuid.LowPart && a.luid.HighPart == desc.AdapterLuid.HighPart)
            {
                continue;
            }
            out.push(Adapter {
                name: crate::util::from_wide(&desc.Description).trim().to_string(),
                luid: desc.AdapterLuid,
                luid_str: luid_string(desc.AdapterLuid),
                dedicated: desc.DedicatedVideoMemory as u64,
            });
        }
    }
    out
}

pub struct GpuSensor {
    pub temperature: Option<f64>,
    pub fan_rpm: Option<u32>,
}

/// Keeps kernel adapter handles open between reads.
#[derive(Default)]
pub struct GpuSensors {
    handles: HashMap<String, u32>,
}

impl GpuSensors {
    pub fn read(&mut self, adapter: &Adapter) -> Option<GpuSensor> {
        let handle = match self.handles.get(&adapter.luid_str) {
            Some(h) => *h,
            None => {
                let mut open = D3DKMT_OPENADAPTERFROMLUID { AdapterLuid: adapter.luid, hAdapter: 0 };
                if unsafe { D3DKMTOpenAdapterFromLuid(&mut open) }.is_err() {
                    return None;
                }
                self.handles.insert(adapter.luid_str.clone(), open.hAdapter);
                open.hAdapter
            }
        };
        let mut perf = D3DKMT_ADAPTER_PERFDATA::default();
        let mut query = D3DKMT_QUERYADAPTERINFO {
            hAdapter: handle,
            Type: KMTQAITYPE_ADAPTERPERFDATA,
            pPrivateDriverData: &mut perf as *mut _ as *mut _,
            PrivateDriverDataSize: size_of::<D3DKMT_ADAPTER_PERFDATA>() as u32,
        };
        if unsafe { D3DKMTQueryAdapterInfo(&mut query) }.is_err() {
            return None;
        }
        // Temperature is reported in deci-degrees Celsius; 0 means "not supported".
        let temperature = (perf.Temperature > 0).then(|| perf.Temperature as f64 / 10.0).filter(|t| *t < 150.0);
        Some(GpuSensor { temperature, fan_rpm: (perf.FanRPM > 0).then_some(perf.FanRPM) })
    }
}

impl Drop for GpuSensors {
    fn drop(&mut self) {
        for (_, h) in self.handles.drain() {
            unsafe {
                let _ = D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER { hAdapter: h });
            }
        }
    }
}

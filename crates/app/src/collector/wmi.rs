//! WMI sensors: the LibreHardwareMonitor / OpenHardwareMonitor bridge and ACPI thermal zones.
//!
//! LibreHardwareMonitor publishes every sensor it reads (CPU package temperature, clocks, fans, voltages, power…)
//! in `ROOT\LibreHardwareMonitor` while it runs. Reading that namespace gives the widget the full sensor set
//! without shipping a kernel driver. `MSAcpi_ThermalZoneTemperature` needs administrator rights.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tm_core::snapshot::SensorInfo;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoInitializeSecurity, CoSetProxyBlanket, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL,
    RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_I4, VT_R4, VT_R8, VT_UI4, VariantClear};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, IWbemServices, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE,
    WbemLocator,
};
use windows::core::{BSTR, HSTRING, PCWSTR};

/// Call once per process before using WMI (ignored if someone already initialized security).
pub fn init_security() {
    unsafe {
        let _ = CoInitializeSecurity(
            None,
            -1,
            None,
            None,
            RPC_C_AUTHN_LEVEL_DEFAULT,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
            None,
        );
    }
}

pub struct Wmi {
    locator: IWbemLocator,
    lhm: Option<IWbemServices>,
    acpi: Option<IWbemServices>,
    lhm_retry: Instant,
    acpi_retry: Instant,
    hardware_names: HashMap<String, String>,
}

fn variant_string(v: &VARIANT) -> Option<String> {
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        (inner.vt == VT_BSTR).then(|| inner.Anonymous.bstrVal.to_string())
    }
}

fn variant_f64(v: &VARIANT) -> Option<f64> {
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        match inner.vt {
            VT_R4 => Some(inner.Anonymous.fltVal as f64),
            VT_R8 => Some(inner.Anonymous.dblVal),
            VT_I4 => Some(inner.Anonymous.lVal as f64),
            VT_UI4 => Some(inner.Anonymous.ulVal as f64),
            _ => None,
        }
    }
}

fn get(obj: &IWbemClassObject, name: &str) -> VARIANT {
    let mut v = VARIANT::default();
    unsafe {
        let _ = obj.Get(&HSTRING::from(name), 0, &mut v, None, None);
    }
    v
}

fn get_string(obj: &IWbemClassObject, name: &str) -> Option<String> {
    let mut v = get(obj, name);
    let s = variant_string(&v);
    unsafe {
        let _ = VariantClear(&mut v);
    }
    s
}

fn get_f64(obj: &IWbemClassObject, name: &str) -> Option<f64> {
    let mut v = get(obj, name);
    let f = variant_f64(&v);
    unsafe {
        let _ = VariantClear(&mut v);
    }
    f
}

fn unit_for(sensor_type: &str) -> &'static str {
    match sensor_type {
        "Temperature" => "°C",
        "Load" | "Control" | "Level" | "Humidity" => "%",
        "Fan" => "RPM",
        "Voltage" => "V",
        "Current" => "A",
        "Power" => "W",
        "Clock" => "MHz",
        "Frequency" => "Hz",
        "Data" => "GB",
        "SmallData" => "MB",
        "Throughput" => "B/s",
        "Energy" => "mWh",
        "Noise" => "dBA",
        "Flow" => "L/h",
        _ => "",
    }
}

impl Wmi {
    pub fn new() -> Option<Self> {
        let locator: IWbemLocator = unsafe { CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER) }.ok()?;
        let now = Instant::now();
        Some(Self { locator, lhm: None, acpi: None, lhm_retry: now, acpi_retry: now, hardware_names: HashMap::new() })
    }

    fn connect(&self, namespace: &str) -> Option<IWbemServices> {
        unsafe {
            let empty = BSTR::new();
            let services = self.locator.ConnectServer(&BSTR::from(namespace), &empty, &empty, &empty, 0, &empty, None).ok()?;
            CoSetProxyBlanket(
                &services,
                RPC_C_AUTHN_WINNT,
                RPC_C_AUTHZ_NONE,
                PCWSTR::null(),
                RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
            )
            .ok()?;
            Some(services)
        }
    }

    fn query(services: &IWbemServices, wql: &str) -> Option<Vec<IWbemClassObject>> {
        unsafe {
            let e = services
                .ExecQuery(&BSTR::from("WQL"), &BSTR::from(wql), WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, None)
                .ok()?;
            let mut out = Vec::new();
            loop {
                let mut row = [None];
                let mut returned = 0u32;
                let hr = e.Next(WBEM_INFINITE, &mut row, &mut returned);
                if hr.is_err() || returned == 0 {
                    break;
                }
                if let Some(obj) = row[0].take() {
                    out.push(obj);
                }
            }
            Some(out)
        }
    }

    /// All sensors from a running LibreHardwareMonitor/OpenHardwareMonitor; empty when neither runs.
    pub fn lhm_sensors(&mut self) -> Vec<SensorInfo> {
        if self.lhm.is_none() {
            if Instant::now() < self.lhm_retry {
                return Vec::new();
            }
            self.lhm_retry = Instant::now() + Duration::from_secs(30);
            self.lhm = self.connect(r"ROOT\LibreHardwareMonitor").or_else(|| self.connect(r"ROOT\OpenHardwareMonitor"));
            self.hardware_names.clear();
        }
        let Some(services) = self.lhm.clone() else { return Vec::new() };

        if self.hardware_names.is_empty()
            && let Some(rows) = Self::query(&services, "SELECT Identifier, Name FROM Hardware")
        {
            for r in rows {
                if let (Some(id), Some(name)) = (get_string(&r, "Identifier"), get_string(&r, "Name")) {
                    self.hardware_names.insert(id, name);
                }
            }
        }

        let Some(rows) = Self::query(&services, "SELECT Identifier, Name, SensorType, Value, Parent FROM Sensor") else {
            // The monitor was closed; reconnect later.
            self.lhm = None;
            return Vec::new();
        };
        if rows.is_empty() {
            self.lhm = None;
        }
        rows.iter()
            .filter_map(|r| {
                let id = get_string(r, "Identifier")?;
                let kind = get_string(r, "SensorType").unwrap_or_default();
                let parent = get_string(r, "Parent").unwrap_or_default();
                Some(SensorInfo {
                    hardware: self.hardware_names.get(&parent).cloned().unwrap_or(parent),
                    name: get_string(r, "Name").unwrap_or_else(|| id.clone()),
                    value: get_f64(r, "Value")?,
                    unit: unit_for(&kind).to_string(),
                    kind,
                    id,
                })
            })
            .collect()
    }

    /// Hottest ACPI thermal zone (°C). Requires administrator rights.
    pub fn acpi_temperature(&mut self) -> Option<f64> {
        if self.acpi.is_none() {
            if Instant::now() < self.acpi_retry {
                return None;
            }
            self.acpi_retry = Instant::now() + Duration::from_secs(60);
            self.acpi = self.connect(r"ROOT\WMI");
        }
        let rows = Self::query(self.acpi.as_ref()?, "SELECT CurrentTemperature FROM MSAcpi_ThermalZoneTemperature")?;
        rows.iter()
            .filter_map(|r| get_f64(r, "CurrentTemperature"))
            .map(|tenth_kelvin| tenth_kelvin / 10.0 - 273.15)
            .filter(|t| (1.0..150.0).contains(t))
            .fold(None, |acc: Option<f64>, t| Some(acc.map_or(t, |a| a.max(t))))
    }
}

/// Picks the CPU package temperature out of LibreHardwareMonitor sensors.
pub fn cpu_temperature(sensors: &[SensorInfo]) -> Option<f64> {
    let cpu = |s: &&SensorInfo| s.kind == "Temperature" && s.id.contains("cpu");
    let preferred = ["CPU Package", "Core (Tctl/Tdie)", "Core Average", "CPU Cores", "Core Max"];
    preferred
        .iter()
        .find_map(|name| sensors.iter().filter(cpu).find(|s| s.name == *name))
        .or_else(|| sensors.iter().filter(cpu).max_by(|a, b| a.value.total_cmp(&b.value)))
        .map(|s| s.value)
}
